//! SecondBot's Discord commands, on the server the bot is connected to (or any server it is in).
//!
//! As SecondBot's: "True"/"False" answers, "[True, n]" for the messages cleared, "[ok, id]" for
//! a role made. Deliberate differences: the lists (Discord_RoleList, Discord_TextChannels_List,
//! Discord_MembersList, Discord_Role_GetSettings) answer as JSON objects (SecondBot prints C#
//! type names); Discord_TextChannels_List, Discord_RoleCreate, Discord_Role_UpdatePerms and
//! Discord_MuteMember take valid ids (SecondBot's checks are inverted and refuse them);
//! Discord_VoiceJoin and Discord_VoiceLeave always fail (no Discord voice in the container).

use super::reply::{Answer, basic, failure, json, json_pairs};
use super::{Ctx, cs_bool, owned};
use crate::discord::Discord;
use std::collections::BTreeMap;
use std::sync::Arc;
use twilight_http::request::AuditLogReason;
use twilight_model::channel::ChannelType;
use twilight_model::guild::Permissions;
use twilight_model::id::Id;

/// Discord's permission bits, by SecondBot's names (its UpdateFlag and RoleSettings).
const PERMS: [(&str, u64); 30] = [
    ("CreateInstantInvite", 1 << 0),
    ("KickMembers", 1 << 1),
    ("BanMembers", 1 << 2),
    ("Administrator", 1 << 3),
    ("ManageChannels", 1 << 4),
    ("ManageGuild", 1 << 5),
    ("AddReactions", 1 << 6),
    ("ViewAuditLog", 1 << 7),
    ("PrioritySpeaker", 1 << 8),
    ("Stream", 1 << 9),
    ("ViewChannel", 1 << 10),
    ("SendMessages", 1 << 11),
    ("SendTTSMessages", 1 << 12),
    ("ManageMessages", 1 << 13),
    ("EmbedLinks", 1 << 14),
    ("AttachFiles", 1 << 15),
    ("ReadMessageHistory", 1 << 16),
    ("MentionEveryone", 1 << 17),
    ("UseExternalEmojis", 1 << 18),
    ("Connect", 1 << 20),
    ("Speak", 1 << 21),
    ("MuteMembers", 1 << 22),
    ("DeafenMembers", 1 << 23),
    ("MoveMembers", 1 << 24),
    ("UseVAD", 1 << 25),
    ("ChangeNickname", 1 << 26),
    ("ManageNicknames", 1 << 27),
    ("ManageRoles", 1 << 28),
    ("ManageWebhooks", 1 << 29),
    ("ManageEmojisAndStickers", 1 << 30),
];

/// RoleSettings' order of the permissions.
const SETTINGS_ORDER: [&str; 30] = [
    "AttachFiles",
    "ReadMessageHistory",
    "MentionEveryone",
    "UseExternalEmojis",
    "Connect",
    "Speak",
    "MuteMembers",
    "UseVAD",
    "MoveMembers",
    "EmbedLinks",
    "PrioritySpeaker",
    "Stream",
    "ChangeNickname",
    "ManageNicknames",
    "ManageRoles",
    "DeafenMembers",
    "ManageMessages",
    "ViewChannel",
    "SendMessages",
    "CreateInstantInvite",
    "BanMembers",
    "SendTTSMessages",
    "Administrator",
    "ManageChannels",
    "KickMembers",
    "AddReactions",
    "ViewAuditLog",
    "ManageWebhooks",
    "ManageGuild",
    "ManageEmojisAndStickers",
];

/// Discord's DarkRed, the colour SecondBot gives a new role.
const DARK_RED: u32 = 0x99_2D_22;

fn tf(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

/// An id as C#'s ulong.TryParse reads it.
fn id<T>(s: &str) -> Option<Id<T>> {
    s.trim().parse::<u64>().ok().and_then(Id::new_checked)
}

/// The connection, when it is ready (DiscordReady).
fn ready(ctx: &Ctx) -> Option<Arc<Discord>> {
    crate::discord::connection(&ctx.discord).filter(|d| d.ready())
}

const NOT_READY: &str = "Discord client not ready";

pub async fn add_role(ctx: &Ctx, server: &str, role: &str, member: &str) -> Answer {
    const C: &str = "Discord_AddRole";
    let Some(d) = ready(ctx) else {
        return failure(C, NOT_READY, &[]);
    };
    let args = owned(&[server, role, member]);
    let ok = match (id(server), id(role), id(member)) {
        (Some(g), Some(r), Some(u)) => d.http.add_guild_member_role(g, u, r).await.is_ok(),
        _ => false,
    };
    basic(C, tf(ok), &args)
}

/// Discord_BanMember: banned, their messages of the last 7 days deleted (BanAsync(7, why)).
pub async fn ban_member(ctx: &Ctx, server: &str, member: &str, why: &str) -> Answer {
    const C: &str = "Discord_BanMember";
    let Some(d) = ready(ctx) else {
        return failure(C, NOT_READY, &[]);
    };
    let args = owned(&[server, member, why]);
    if why.trim().is_empty() {
        return failure(C, "Why empty", &args);
    }
    let ok = match (id(server), id(member)) {
        (Some(g), Some(u)) => {
            let req = d
                .http
                .create_ban(g, u)
                .delete_message_seconds(7 * 86_400)
                .reason(why);
            req.await.is_ok()
        }
        _ => false,
    };
    basic(C, tf(ok), &args)
}

pub async fn member_roles(ctx: &Ctx, server: &str, member: &str) -> Answer {
    const C: &str = "Discord_GetMemberRoles";
    let Some(d) = ready(ctx) else {
        return failure(C, NOT_READY, &[]);
    };
    let Some(g) = id(server) else {
        return failure(C, "invaild server", &[]);
    };
    let Some(u) = id(member) else {
        return failure(C, "invaild member", &[]);
    };
    let found = match d.http.guild_member(g, u).await {
        Ok(r) => r.model().await.ok(),
        Err(_) => None,
    };
    match found {
        Some(m) => {
            let roles: Vec<String> = m.roles.iter().map(ToString::to_string).collect();
            basic(C, &json(&roles), &[])
        }
        None => failure(C, "Cant get user", &[]),
    }
}

/// Discord_BulkClear_Messages: a member's messages of the last 13 days 22:59:59 (Discord's bulk
/// delete takes two weeks at most), in each text channel's last 100.
pub async fn bulk_clear(ctx: &Ctx, server: &str, member: &str) -> Answer {
    const C: &str = "Discord_BulkClear_Messages";
    let Some(d) = ready(ctx) else {
        return failure(C, NOT_READY, &[]);
    };
    let args = owned(&[server, member]);
    let (Some(g), Some(u)) = (
        id(server),
        id::<twilight_model::id::marker::UserMarker>(member),
    ) else {
        return basic(C, "[False, 0]", &args);
    };
    let Ok(channels) = d.http.guild_channels(g).await else {
        return basic(C, "[False, 0]", &args);
    };
    let channels = channels.models().await.unwrap_or_default();
    let limit = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |t| t.as_secs() as i64)
        - (13 * 86_400 + 22 * 3600 + 59 * 60 + 59);
    let mut deleted = 0;
    for c in channels.iter().filter(|c| c.kind == ChannelType::GuildText) {
        let Ok(r) = d.http.channel_messages(c.id).limit(100).await else {
            continue;
        };
        let ids: Vec<_> = r
            .models()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|m| m.author.id == u && m.timestamp.as_secs() > limit)
            .map(|m| m.id)
            .collect();
        let gone = match ids.len() {
            0 => false,
            1 => d.http.delete_message(c.id, ids[0]).await.is_ok(),
            _ => d.http.delete_messages(c.id, &ids).await.is_ok(),
        };
        if gone {
            deleted += ids.len();
        }
    }
    basic(C, &format!("[True, {deleted}]"), &args)
}

/// Discord_Dm_Member: a direct message to a member of the server.
pub async fn dm_member(ctx: &Ctx, server: &str, member: &str, message: &str) -> Answer {
    const C: &str = "Discord_Dm_Member";
    let Some(d) = ready(ctx) else {
        return failure(C, NOT_READY, &[]);
    };
    let args = owned(&[server, member, message]);
    let ok = match (id(server), id(member)) {
        (Some(g), Some(u)) => {
            let member_there = d.http.guild_member(g, u).await.is_ok();
            let channel = if member_there {
                match d.http.create_private_channel(u).await {
                    Ok(r) => r.model().await.ok().map(|c| c.id),
                    Err(_) => None,
                }
            } else {
                None
            };
            match channel {
                Some(c) => d.post(c, message).await.is_some(),
                None => false,
            }
        }
        _ => false,
    };
    basic(C, tf(ok), &args)
}

/// Discord_MembersList: `{id: "nickname|username"}`, or the username without a nickname. It
/// needs the SERVER MEMBERS intent turned on in the Discord application.
pub async fn members_list(ctx: &Ctx, server: &str) -> Answer {
    const C: &str = "Discord_MembersList";
    let Some(d) = ready(ctx) else {
        return failure(C, NOT_READY, &[]);
    };
    let args = owned(&[server]);
    let Some(g) = id(server) else {
        return failure(C, "invaild server", &args);
    };
    let mut out = BTreeMap::new();
    let mut after = None;
    loop {
        let mut req = d.http.guild_members(g).limit(1000);
        if let Some(a) = after {
            req = req.after(a);
        }
        let page = match req.await {
            Ok(r) => r.models().await.unwrap_or_default(),
            Err(e) => {
                log::warn!("{C}: {e}");
                return failure(C, "Unable to list members", &args);
            }
        };
        for m in &page {
            let shown = match &m.nick {
                Some(nick) => format!("{nick}|{}", m.user.name),
                None => m.user.name.clone(),
            };
            out.insert(m.user.id.to_string(), shown);
        }
        if page.len() < 1000 {
            break;
        }
        after = page.last().map(|m| m.user.id);
    }
    basic(C, &json(&out), &args)
}

/// Discord_MessageChannel: a message in a text channel of the server, read aloud or not.
pub async fn message_channel(
    ctx: &Ctx,
    server: &str,
    channel: &str,
    tts: &str,
    message: &str,
) -> Answer {
    const C: &str = "Discord_MessageChannel";
    let Some(d) = ready(ctx) else {
        return failure(C, NOT_READY, &[]);
    };
    let args = owned(&[server, channel, tts, message]);
    if message.trim().is_empty() {
        return failure(C, "message empty", &args);
    }
    let Some(tts) = cs_bool(tts) else {
        return basic(C, "False", &args);
    };
    let (Some(g), Some(c)) = (id(server), id(channel)) else {
        return basic(C, "False", &args);
    };
    // the channel must be one of the server's text channels (SendMessageToDiscord)
    let in_server = match d.http.guild_channels(g).await {
        Ok(r) => r
            .models()
            .await
            .unwrap_or_default()
            .iter()
            .any(|x| x.id == c && x.kind == ChannelType::GuildText),
        Err(_) => false,
    };
    let text = crate::discord::fit(message);
    let ok = in_server
        && d.http
            .create_message(c)
            .content(&text)
            .tts(tts)
            .await
            .is_ok();
    basic(C, tf(ok), &args)
}

/// Discord_MuteMember: server-muted in voice, or not.
pub async fn mute_member(ctx: &Ctx, server: &str, member: &str, mode: &str) -> Answer {
    const C: &str = "Discord_MuteMember";
    let Some(d) = ready(ctx) else {
        return failure(C, NOT_READY, &[]);
    };
    let args = owned(&[server, member, mode]);
    let mute = cs_bool(mode).unwrap_or(false);
    let ok = match (id(server), id(member)) {
        (Some(g), Some(u)) => d.http.update_guild_member(g, u).mute(mute).await.is_ok(),
        _ => false,
    };
    basic(C, tf(ok), &args)
}

/// Discord_Role_GetSettings: SecondBot's RoleSettings, in its order.
pub async fn role_settings(ctx: &Ctx, server: &str, role: &str) -> Answer {
    const C: &str = "Discord_Role_GetSettings";
    let Some(d) = ready(ctx) else {
        return failure(C, NOT_READY, &[]);
    };
    let args = owned(&[server, role]);
    let (Some(g), Some(r)) = (id(server), id(role)) else {
        return failure(C, "Unable to find role", &args);
    };
    let roles = match d.http.roles(g).await {
        Ok(x) => x.models().await.unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let Some(found) = roles.into_iter().find(|x| x.id == r) else {
        return failure(C, "Unable to find role", &args);
    };
    let assigned = match d.http.role_member_counts(g).await {
        Ok(x) => x
            .model()
            .await
            .ok()
            .and_then(|m| m.get(&r).copied())
            .unwrap_or(0),
        Err(_) => 0,
    };
    let c = found.colors.primary_color;
    let mut pairs: Vec<(String, String)> = vec![
        (
            "Config|Color".into(),
            format!("{},{},{}", (c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff),
        ),
        ("Config|Name".into(), found.name.clone()),
        ("Info|AssignedCount".into(), assigned.to_string()),
    ];
    let bits = found.permissions.bits();
    for name in SETTINGS_ORDER {
        let bit = PERMS
            .iter()
            .find(|(n, _)| *n == name)
            .map_or(0, |(_, b)| *b);
        pairs.push((format!("Perm|{name}"), tf(bits & bit != 0).to_owned()));
    }
    let borrowed: Vec<(&str, String)> =
        pairs.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
    basic(C, &json_pairs(&borrowed), &args)
}

/// Discord_Role_UpdatePerms: `Speak=True,SendMessages=False`, SecondBot's names.
pub async fn role_update_perms(ctx: &Ctx, server: &str, role: &str, flags: &str) -> Answer {
    const C: &str = "Discord_Role_UpdatePerms";
    let Some(d) = ready(ctx) else {
        return failure(C, NOT_READY, &[]);
    };
    let args = owned(&[server, role, flags]);
    let (Some(g), Some(r)) = (id(server), id(role)) else {
        return basic(C, "False", &args);
    };
    let current = match d.http.roles(g).await {
        Ok(x) => x
            .models()
            .await
            .unwrap_or_default()
            .into_iter()
            .find(|x| x.id == r)
            .map(|x| x.permissions.bits()),
        Err(_) => None,
    };
    let Some(mut bits) = current else {
        return basic(C, "False", &args);
    };
    for part in flags.split(',') {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        let (Some((_, bit)), Some(on)) = (
            PERMS.iter().find(|(n, _)| *n == name.trim()),
            cs_bool(value),
        ) else {
            continue;
        };
        if on {
            bits |= bit;
        } else {
            bits &= !bit;
        }
    }
    let perms = Permissions::from_bits_truncate(bits);
    let ok = d.http.update_role(g, r).permissions(perms).await.is_ok();
    basic(C, tf(ok), &args)
}

/// Discord_RoleCreate: "[ok, id]", as SecondBot's pair prints.
pub async fn role_create(ctx: &Ctx, server: &str, role: &str) -> Answer {
    const C: &str = "Discord_RoleCreate";
    let Some(d) = ready(ctx) else {
        return failure(C, NOT_READY, &[]);
    };
    let args = owned(&[server, role]);
    let Some(g) = id(server) else {
        return basic(C, "[Unable to process server id, 0]", &args);
    };
    let made = match d
        .http
        .create_role(g)
        .name(role)
        .color(DARK_RED)
        .permissions(Permissions::empty())
        .await
    {
        Ok(r) => r.model().await.ok().map(|x| x.id),
        Err(e) => {
            log::warn!("{C}: {e}");
            None
        }
    };
    match made {
        Some(id) => basic(C, &format!("[ok, {id}]"), &args),
        None => basic(C, "[Unable to create role, 0]", &args),
    }
}

/// Discord_RoleList: `{id: name}`.
pub async fn role_list(ctx: &Ctx, server: &str) -> Answer {
    const C: &str = "Discord_RoleList";
    let Some(d) = ready(ctx) else {
        return failure(C, NOT_READY, &[]);
    };
    let args = owned(&[server]);
    let Some(g) = id(server) else {
        return failure(C, "invaild server", &args);
    };
    match d.http.roles(g).await {
        Ok(r) => {
            let list: BTreeMap<String, String> = r
                .models()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|x| (x.id.to_string(), x.name))
                .collect();
            basic(C, &json(&list), &args)
        }
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

pub async fn role_remove(ctx: &Ctx, server: &str, role: &str) -> Answer {
    const C: &str = "Discord_RoleRemove";
    let Some(d) = ready(ctx) else {
        return failure(C, NOT_READY, &[]);
    };
    let args = owned(&[server, role]);
    let ok = match (id(server), id(role)) {
        (Some(g), Some(r)) => d.http.delete_role(g, r).await.is_ok(),
        _ => false,
    };
    basic(C, tf(ok), &args)
}

/// Discord_TextChannels_List: `{id: name}` of the server's text channels.
pub async fn text_channels(ctx: &Ctx, server: &str) -> Answer {
    const C: &str = "Discord_TextChannels_List";
    let Some(d) = ready(ctx) else {
        return failure(C, NOT_READY, &[]);
    };
    let Some(g) = id(server) else {
        return failure(C, "invaild server", &[]);
    };
    match d.http.guild_channels(g).await {
        Ok(r) => {
            let list: BTreeMap<String, String> = r
                .models()
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|c| c.kind == ChannelType::GuildText)
                .map(|c| (c.id.to_string(), c.name.unwrap_or_default()))
                .collect();
            basic(C, &json(&list), &[])
        }
        Err(e) => failure(C, &e.to_string(), &[]),
    }
}

/// Discord voice needs audio libraries the container does not carry.
pub fn voice_join(ctx: &Ctx, channel: &str) -> Answer {
    const C: &str = "Discord_VoiceJoin";
    let args = owned(&[channel]);
    if ready(ctx).is_none() {
        return failure(C, NOT_READY, &args);
    }
    failure(C, "Voice service is not enabled", &args)
}

pub fn voice_leave() -> Answer {
    failure("Discord_VoiceLeave", "Not in a Discord voice channel", &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_has_its_bit() {
        for name in SETTINGS_ORDER {
            assert!(PERMS.iter().any(|(n, _)| *n == name), "{name}");
        }
        assert_eq!(PERMS.len(), SETTINGS_ORDER.len());
    }

    #[test]
    fn ids_as_ulong_parses_them() {
        assert!(id::<twilight_model::id::marker::GuildMarker>("1234").is_some());
        assert!(id::<twilight_model::id::marker::GuildMarker>("0").is_none());
        assert!(id::<twilight_model::id::marker::GuildMarker>("x").is_none());
    }
}
