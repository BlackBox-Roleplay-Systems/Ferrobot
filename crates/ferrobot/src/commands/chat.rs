//! SecondBot's Chat commands, and the Groupchat ones of its Group commands: local chat, IMs,
//! group chat, and the history the bot keeps (history.rs).

use super::reply::{Answer, basic, failure, json};
use super::{Ctx, avatar, key, owned};
use ferro_client::events::chat_type;

/// C#'s bool.ToString.
fn cs_bool(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

pub fn local_chat_history(ctx: &Ctx) -> Answer {
    basic("LocalChatHistory", &json(&ctx.history.local()), &[])
}

/// Say, Shout, Whisper: on a channel; the bot's own line goes in the history.
pub async fn speak(ctx: &Ctx, command: &str, channel: &str, message: &str) -> Answer {
    let args = owned(&[channel, message]);
    if message.trim().is_empty() {
        return failure(command, "Message empty", &args);
    }
    let Ok(channel) = channel.trim().parse::<i32>() else {
        return failure(command, "Invaild channel", &args);
    };
    let kind = match command {
        "Shout" => chat_type::SHOUT,
        "Whisper" => chat_type::WHISPER,
        _ => chat_type::SAY,
    };
    let name = ctx.client.state().name;
    ctx.history.local_line(format!("{{BOT}} {name}: {message}"));
    match ctx.client.chat(message, channel, kind).await {
        Ok(()) => basic(command, "ok", &[]),
        Err(e) => failure(command, &e.to_string(), &args),
    }
}

/// IM: to an avatar by key or name; its window opens with the bot's line.
pub async fn im(ctx: &Ctx, who: &str, message: &str) -> Answer {
    const C: &str = "IM";
    let args = owned(&[who, message]);
    let Some(id) = avatar(&ctx.client, who).await else {
        return failure(C, "avatar lookup", &args);
    };
    if message.trim().is_empty() {
        return failure(C, "Message empty", &args);
    }
    let name = ctx.client.name(id).await.ok().flatten().unwrap_or_default();
    ctx.history.open(id, false, &name);
    ctx.history.record(id, "Bot", message, true);
    match ctx.client.im(id, message).await {
        Ok(()) => basic(C, "ok", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

pub fn chat_windows(ctx: &Ctx) -> Answer {
    basic("chatwindows", &json(&ctx.history.im_windows()), &[])
}

pub fn list_with_unread(ctx: &Ctx) -> Answer {
    let ids: Vec<String> = ctx
        .history
        .im_unread()
        .iter()
        .map(ToString::to_string)
        .collect();
    basic("listwithunread", &json(&ids), &[])
}

pub fn have_unread_ims(ctx: &Ctx) -> Answer {
    basic(
        "haveunreadims",
        cs_bool(!ctx.history.im_unread().is_empty()),
        &[],
    )
}

pub fn get_im_chat(ctx: &Ctx, avatar: &str) -> Answer {
    const C: &str = "getimchat";
    let Some(id) = key(avatar) else {
        return failure(C, "avatar UUID invaild", &[]);
    };
    basic(C, &json(&ctx.history.read(id, false)), &[])
}

pub fn group_chat_unread_groups(ctx: &Ctx) -> Answer {
    let ids: Vec<String> = ctx
        .history
        .group_unread()
        .iter()
        .map(ToString::to_string)
        .collect();
    basic("GroupchatListAllUnreadGroups", &json(&ids), &[])
}

pub fn group_chat_has_unread(ctx: &Ctx, group: &str) -> Answer {
    const C: &str = "GroupchatGroupHasUnread";
    let args = owned(&[group]);
    let Some(group) = key(group) else {
        return failure(C, "group value is invaild", &args);
    };
    if !ctx.client.state().groups.iter().any(|g| g.id == group) {
        return failure(C, "Unknown group", &args);
    }
    basic(C, cs_bool(ctx.history.group_has_unread(group)), &[])
}

pub fn group_chat_any_unread(ctx: &Ctx) -> Answer {
    basic(
        "GroupchatAnyUnread",
        cs_bool(!ctx.history.group_unread().is_empty()),
        &[],
    )
}

pub fn group_chat_clear_all(ctx: &Ctx) -> Answer {
    ctx.history.clear_groups();
    basic("GroupchatClearAll", "ok", &[])
}

pub fn group_chat_history(ctx: &Ctx, group: &str) -> Answer {
    const C: &str = "GroupchatHistory";
    let Some(id) = key(group) else {
        return failure(C, "Group UUID invaild", &owned(&[group]));
    };
    basic(C, &json(&ctx.history.read(id, true)), &[])
}

/// Groupchat: a line in the group's chat, joining it first.
pub async fn group_chat(ctx: &Ctx, group: &str, message: &str) -> Answer {
    const C: &str = "Groupchat";
    let args = owned(&[group, message]);
    let Some(id) = key(group) else {
        return failure(C, "group value is invaild", &args);
    };
    if !ctx.client.state().groups.iter().any(|g| g.id == id) {
        return failure(C, "Unknown group", &args);
    }
    ctx.history.open(id, true, "");
    ctx.history.record(id, "Bot", message, true);
    match ctx.client.group_chat_send(id, message).await {
        Ok(()) => basic(C, "Sending", &[]),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}
