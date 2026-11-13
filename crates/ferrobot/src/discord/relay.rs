//! What goes between Second Life and the bot's Discord channels, as SecondBot's DiscordService:
//! local chat, object IMs and the bot's state to the `bot` channels, an avatar's IMs to its
//! channel in `im`, a group's chat to its channel in `group`; and back, what a Discord master
//! writes there, with the channels' `!` commands.

use super::{BOT, Discord, GROUP, IM, Known};
use crate::commands::{self, Ctx};
use ferro_client::events::{ChatMessage, InstantMessage, chat_type, dialog};
use ferro_client::{Client, Event};
use std::sync::Arc;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use twilight_http::request::channel::reaction::RequestReactionType;
use twilight_model::channel::Message;
use uuid::Uuid;

pub const STATUS_TOPIC: &str = "State of the bot service";
pub const LOCAL_TOPIC: &str = "Talk and view localchat !clear";

/// GroupPrefill: the group's key, then what can be written there.
fn group_topic(group: Uuid) -> String {
    format!(
        "group={group}||| !clear !notice {{Title}}@@@{{Message}}[@@@{{Inventory UUID}}] !eject {{Avatar}} !invite {{Avatar}} !ban {{Avatar}}"
    )
}

fn avatar_topic(avatar: Uuid) -> String {
    format!("avatar={avatar}||| !close !offertp !clear")
}

/// The key a channel's topic carries: `avatar=<key>|||…` or `group=<key>|||…`.
fn topic_key(topic: &str, kind: &str) -> Option<Uuid> {
    let (head, _) = topic.split_once("|||")?;
    let (k, v) = head.split_once('=')?;
    (k == kind).then(|| Uuid::parse_str(v.trim()).ok())?
}

/// Names the grid uses for itself, never relayed (hard_blocked_agents).
fn blocked(name: &str) -> bool {
    matches!(name.to_lowercase().as_str(), "secondlife" | "second life")
}

/// The emoji SecondBot puts before a line of local chat.
fn source_mark(c: &ChatMessage, me: &str) -> &'static str {
    // ChatType: RegionSayTo is 7
    if c.chat_type == chat_type::OWNER_SAY {
        ":robot:"
    } else if c.chat_type == 7 {
        ":dart:"
    } else if c.source_type == 2 {
        ":diamond_shape_with_a_dot_inside:"
    } else if c.source_type == 0 {
        ":comet:"
    } else if c.from_name == me {
        ":disguised_face:"
    } else {
        ":person_bald:"
    }
}

async fn local(discord: &Discord, line: &str) {
    if let Some(id) = discord.channel("localchat", LOCAL_TOPIC, BOT).await {
        discord.post(id, line).await;
    }
}

async fn status(discord: &Discord, line: &str) {
    if let Some(id) = discord.channel("status", STATUS_TOPIC, BOT).await {
        discord.post(id, line).await;
    }
}

/// Connected and set up: the bot's state, and a channel for each of its groups (GroupCurrent).
pub async fn on_ready(ctx: &Ctx, discord: &Discord) {
    let st = ctx.client.state();
    match st.region.as_ref().filter(|_| st.online) {
        Some(r) => {
            discord.presence(&r.name, false);
            status(discord, &format!("In world: {} in {}", st.name, r.name)).await;
            for g in &st.groups {
                discord.channel(&g.name, &group_topic(g.id), GROUP).await;
            }
        }
        None => {
            discord.presence("Waiting for login", true);
            status(discord, "Waiting for login").await;
        }
    }
}

/// What the bot hears in Second Life, to Discord.
pub async fn from_world(client: Client, discord: Arc<Discord>, mut events: Receiver<Event>) {
    loop {
        let event = match events.recv().await {
            Ok(e) => e,
            Err(RecvError::Lagged(n)) => {
                log::warn!("discord: {n} events missed");
                continue;
            }
            Err(RecvError::Closed) => return,
        };
        if !discord.ready() {
            continue;
        }
        match event {
            Event::Chat(c) => {
                let kinds = [
                    chat_type::WHISPER,
                    chat_type::SAY,
                    chat_type::SHOUT,
                    chat_type::OWNER_SAY,
                    7,
                ];
                if !kinds.contains(&c.chat_type) || blocked(&c.from_name) || c.message.is_empty() {
                    continue;
                }
                let me = client.state().name.clone();
                let mark = source_mark(&c, &me);
                local(&discord, &format!("{mark}{}: {}", c.from_name, c.message)).await;
            }
            Event::InstantMessage(im) => on_im(&client, &discord, &im).await,
            Event::GroupChat {
                group,
                from_name,
                message,
                ..
            } => {
                let name = client
                    .state()
                    .groups
                    .iter()
                    .find(|g| g.id == group)
                    .map(|g| g.name.clone());
                // SecondBot waits for the group's name ("lookup")
                if let Some(name) = name
                    && let Some(id) = discord.channel(&name, &group_topic(group), GROUP).await
                {
                    discord.post(id, &format!("{from_name}: {message}")).await;
                }
            }
            Event::Arrived { region, .. } => {
                discord.presence(&region, false);
                let name = client.state().name.clone();
                status(&discord, &format!("In world: {name} in {region}")).await;
            }
            // the groups known (GroupCurrent): a channel for each
            Event::GroupsChanged => {
                let groups = client.state().groups.clone();
                for g in &groups {
                    discord.channel(&g.name, &group_topic(g.id), GROUP).await;
                }
            }
            Event::Disconnected { reason } => {
                discord.presence("Waiting for login", true);
                status(&discord, &format!("Disconnected: {reason}")).await;
            }
            _ => {}
        }
    }
}

/// BotImMessage: an object's to `localchat`, an avatar's (or a conference's) to its channel;
/// a group's chat comes as Event::GroupChat.
async fn on_im(client: &Client, discord: &Discord, im: &InstantMessage) {
    match im.dialog {
        dialog::FROM_TASK => {
            local(
                discord,
                &format!(":white_small_square: {}: {}", im.from_name, im.message),
            )
            .await;
        }
        dialog::NOTHING_SPECIAL | dialog::SESSION_SEND => {
            let group = client.state().groups.iter().any(|g| g.id == im.id);
            if (im.dialog == dialog::SESSION_SEND && group) || im.message.is_empty() {
                return;
            }
            let name = im.from_name.to_lowercase().replace(' ', "");
            let topic = avatar_topic(im.from_agent_id);
            if let Some(id) = discord.channel(&name, &topic, IM).await {
                discord
                    .post(id, &format!("{}:{}", im.from_name, im.message))
                    .await;
            }
        }
        _ => {}
    }
}

async fn react(discord: &Discord, m: &Message, ok: bool) {
    let emoji = RequestReactionType::Unicode {
        name: if ok { "✅" } else { "❌" },
    };
    let _ = discord
        .http
        .create_reaction(m.channel_id, m.id, &emoji)
        .await;
}

/// ReplyToMessage: an answer to the message, cut to Discord's limit.
async fn reply(discord: &Discord, m: &Message, text: &str) {
    let text = super::fit(text);
    let _ = discord
        .http
        .create_message(m.channel_id)
        .content(&text)
        .reply(m.id)
        .await;
}

async fn delete(discord: &Discord, m: &Message) {
    let _ = discord.http.delete_message(m.channel_id, m.id).await;
}

/// A command run for a Discord master: its answer as the reply, ✅ or ❌.
async fn run(ctx: &Ctx, discord: &Discord, m: &Message, command: &str, args: &[String]) {
    let (_, answer) = commands::run(ctx, command, args).await;
    let ok = answer.contains(r#""status":true"#);
    react(discord, m, ok).await;
    reply(discord, m, &answer).await;
}

/// The message with the speaker's name before it, unless hidden (hideChatterName).
fn spoken(discord: &Discord, m: &Message) -> String {
    if discord.settings.hide_names {
        m.content.clone()
    } else {
        format!("{}: {}", m.author.name, m.content)
    }
}

/// SecondBot's MessageInteractionEvent: a message starting with the word, in the channel, is
/// posted to the address with a signature (sha1 of the time, the message and the shared
/// secret); ✅ when it went.
async fn interaction(ctx: &Ctx, discord: &Discord, m: &Message) -> bool {
    let Some(i) = &discord.settings.interaction else {
        return false;
    };
    if m.channel_id.get() != i.channel || !m.content.starts_with(&i.name) {
        return false;
    }
    let message = m.content.replace(&i.name, "");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let secret = discord.secret.clone().unwrap_or_default();
    let hash = crate::api::signing::sha1_hex(&format!("{now}{message}{secret}"));
    let form = [
        ("message", message.as_str()),
        ("username", m.author.name.as_str()),
        ("userid", &m.author.id.to_string()),
        ("unixtime", &now.to_string()),
        ("hash", &hash),
    ];
    let body: Vec<String> = form
        .iter()
        .map(|(k, v)| format!("{k}={}", crate::smart_reply::form_encode(v)))
        .collect();
    let sent = ctx
        .http
        .post(&i.url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .timeout(std::time::Duration::from_secs(30))
        .body(body.join("&"))
        .send()
        .await;
    let ok = match sent {
        Ok(_) => true,
        Err(e) => {
            log::warn!("discord interaction: {}", e.without_url());
            false
        }
    };
    react(discord, m, ok).await;
    true
}

/// A message on the server (DiscordClientMessageReceived).
pub async fn from_discord(ctx: &Ctx, discord: &Discord, m: Message) {
    if !discord.ready() || m.guild_id != Some(discord.server()) {
        return;
    }
    if interaction(ctx, discord, &m).await {
        return;
    }
    if Some(m.author.id) == discord.me() {
        return;
    }
    let Some(channel) = discord.learn(m.channel_id).await else {
        return;
    };
    let Some(category) = channel.category.clone() else {
        return;
    };
    let master = discord.is_master(m.author.id);
    let online = ctx.client.state().online;
    match category.as_str() {
        BOT => from_bot_channel(ctx, discord, &channel, &m, master, online).await,
        IM => from_im_channel(ctx, discord, &channel, &m, master, online).await,
        GROUP => from_group_channel(ctx, discord, &channel, &m, master, online).await,
        _ => {}
    }
}

/// The `bot` channels: `status` is the bot's alone, `commands` runs a command (signed form, as
/// by IM), `localchat` says it in local chat.
async fn from_bot_channel(
    ctx: &Ctx,
    discord: &Discord,
    channel: &Known,
    m: &Message,
    master: bool,
    online: bool,
) {
    match channel.name.as_str() {
        "status" => delete(discord, m).await,
        "commands" => {
            if !discord.settings.commands || !master {
                react(discord, m, false).await;
                return;
            }
            if !online {
                delete(discord, m).await;
                return;
            }
            match crate::services::im_commands::unpack(&m.content) {
                Some(u) => run(ctx, discord, m, &u.command, &u.args).await,
                None => {
                    react(discord, m, false).await;
                    reply(discord, m, "Not a command").await;
                }
            }
        }
        "localchat" => {
            if !master {
                react(discord, m, false).await;
                return;
            }
            if !online {
                delete(discord, m).await;
                return;
            }
            if m.content.trim() == "!clear" {
                discord.clean(channel.id).await;
                return;
            }
            let _ = ctx.client.chat(&m.content, 0, chat_type::SAY).await;
            delete(discord, m).await;
        }
        _ => {}
    }
}

/// An avatar's channel: `!close`, `!clear`, `!offertp`, or an IM to them.
async fn from_im_channel(
    ctx: &Ctx,
    discord: &Discord,
    channel: &Known,
    m: &Message,
    master: bool,
    online: bool,
) {
    if !master {
        react(discord, m, false).await;
        return;
    }
    if !online {
        delete(discord, m).await;
        return;
    }
    let Some(avatar) = topic_key(&channel.topic, "avatar") else {
        return;
    };
    match m.content.trim() {
        "!close" => {
            let _ = discord.http.delete_channel(channel.id).await;
            discord.forget(channel.id);
        }
        "!clear" => discord.clean(channel.id).await,
        "!offertp" => {
            let ok = ctx.client.offer_teleport(avatar, "").await.is_ok();
            react(discord, m, ok).await;
        }
        _ => {
            let ok = ctx.client.im(avatar, &spoken(discord, m)).await.is_ok();
            react(discord, m, ok).await;
        }
    }
}

/// A group's channel: `!clear`, `!notice Title@@@Message[@@@item]`, `!eject`, `!invite`,
/// `!ban`, or a line in the group's chat.
async fn from_group_channel(
    ctx: &Ctx,
    discord: &Discord,
    channel: &Known,
    m: &Message,
    master: bool,
    online: bool,
) {
    if !master {
        react(discord, m, false).await;
        return;
    }
    if !online {
        delete(discord, m).await;
        return;
    }
    let Some(group) = topic_key(&channel.topic, "group") else {
        return;
    };
    if !ctx.client.state().groups.iter().any(|g| g.id == group) {
        react(discord, m, false).await;
        reply(discord, m, "Unknown group [Maybe im still loading]").await;
        return;
    }
    let text = m.content.trim();
    let g = group.to_string();
    if text == "!clear" {
        discord.clean(channel.id).await;
    } else if let Some(rest) = text.strip_prefix("!notice") {
        let parts: Vec<String> = rest
            .trim()
            .split("@@@")
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect();
        if parts.len() < 2 {
            react(discord, m, false).await;
            reply(
                discord,
                m,
                "Notices require title and a message split with @@@!",
            )
            .await;
            return;
        }
        let command = if parts.len() == 3 {
            "GroupnoticeWithAttachment"
        } else {
            "Groupnotice"
        };
        let mut args = vec![g];
        args.extend(parts.into_iter().take(3));
        run(ctx, discord, m, command, &args).await;
    } else if let Some(who) = text.strip_prefix("!eject") {
        run(ctx, discord, m, "GroupEject", &[g, who.trim().to_owned()]).await;
    } else if let Some(who) = text.strip_prefix("!invite") {
        let args = [g, who.trim().to_owned(), "everyone".to_owned()];
        run(ctx, discord, m, "GroupInvite", &args).await;
    } else if let Some(who) = text.strip_prefix("!ban") {
        let args = [g, who.trim().to_owned(), "true".to_owned()];
        run(ctx, discord, m, "GroupBan", &args).await;
    } else {
        let _ = ctx.client.group_chat_send(group, &spoken(discord, m)).await;
        // the line comes back from the group's chat
        delete(discord, m).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_in_the_topics() {
        let id = Uuid::from_u128(5);
        assert_eq!(topic_key(&avatar_topic(id), "avatar"), Some(id));
        assert_eq!(topic_key(&group_topic(id), "group"), Some(id));
        assert_eq!(topic_key(&group_topic(id), "avatar"), None);
        assert_eq!(topic_key("a channel", "group"), None);
    }

    #[test]
    fn the_grid_is_not_relayed() {
        assert!(blocked("Second Life"));
        assert!(!blocked("Some Resident"));
    }
}
