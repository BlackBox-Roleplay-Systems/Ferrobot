//! SecondBot's Group commands, the ones BB's SL group sync sends. The texts, the order of the
//! checks and the `args` of each answer are SecondBot's (Commands/Group.cs).
//!
//! Deliberate differences, noted in TASKS.md:
//! - member and role lists are fetched when asked (SecondBot answers from what it kept since the
//!   login); when the grid does not answer in time, the answer is the failure "Updating", never an
//!   empty list a sync could take for a group everyone left;
//! - GroupEject says "Not in group" or "Missing group Eject power" when the grid refuses, where
//!   SecondBot answers "Accepted" without waiting; GroupAddRole refuses an unknown avatar.

use super::reply::{Answer, basic, failure, json};
use super::{avatar, key, owned};
use ferro_client::{Client, ClientError};
use std::collections::BTreeMap;
use uuid::Uuid;

/// A client error as a failure text.
fn why(e: &ClientError) -> &'static str {
    match e {
        ClientError::UnknownGroup => "Unknown group",
        ClientError::NotConnected => "No bot connected yet",
        ClientError::Timeout | ClientError::Refused(_) => "Updating",
    }
}

fn known_group(client: &Client, group: Uuid) -> bool {
    client.state().groups.iter().any(|g| g.id == group)
}

/// GetGroupList: `{group: name}` of the groups the bot is in.
pub fn get_group_list(client: &Client) -> Answer {
    let groups: BTreeMap<String, String> = client
        .state()
        .groups
        .iter()
        .map(|g| (g.id.to_string(), g.name.clone()))
        .collect();
    basic("GetGroupList", &json(&groups), &[])
}

/// GetGroupRoles: `{role: name}`, "everyone" being the nil key.
pub async fn get_group_roles(client: &Client, group: &str) -> Answer {
    const C: &str = "GetGroupRoles";
    let args = owned(&[group]);
    let Some(group) = key(group) else {
        return failure(C, "Invaild group UUID", &args);
    };
    if !known_group(client, group) {
        return failure(C, "Unknown group", &args);
    }
    match client.group_roles(group).await {
        Ok(roles) => {
            let map: BTreeMap<String, String> = roles
                .iter()
                .map(|r| (r.id.to_string(), r.name.clone()))
                .collect();
            basic(C, &json(&map), &[])
        }
        Err(e) => failure(C, why(&e), &args),
    }
}

/// GetGroupMembers: the members' keys.
pub async fn get_group_members(client: &Client, group: &str) -> Answer {
    const C: &str = "GetGroupMembers";
    let args = owned(&[group]);
    let Some(group) = key(group) else {
        return failure(C, "Invaild group UUID", &args);
    };
    match client.group_members(group).await {
        Ok(members) => {
            let ids: Vec<String> = members.iter().map(|m| m.id.to_string()).collect();
            basic(C, &json(&ids), &[])
        }
        Err(e) => failure(C, why(&e), &args),
    }
}

/// GetGroupRoleMembers: the keys of the members holding the role.
pub async fn get_group_role_members(client: &Client, group: &str, role: &str) -> Answer {
    const C: &str = "GetGroupRoleMembers";
    let args = owned(&[group, role]);
    let Some(group) = key(group) else {
        return failure(C, "Invaild group UUID", &args);
    };
    let Some(role) = key(role) else {
        return failure(C, "Invaild role UUID", &args);
    };
    if !known_group(client, group) {
        return failure(C, "Unknown group", &args);
    }
    let roles = match client.group_roles(group).await {
        Ok(roles) => roles,
        Err(_) => return failure(C, "No reply from the grid [Retry later]", &args),
    };
    if !roles.iter().any(|r| r.id == role) {
        return failure(C, "Unknown role", &args);
    }
    let pairs = match client.group_role_members(group).await {
        Ok(pairs) => pairs,
        Err(_) => return failure(C, "No reply from the grid [Retry later]", &args),
    };
    let mut members: Vec<String> = Vec::new();
    for (r, member) in pairs {
        let id = member.to_string();
        if r == role && !members.contains(&id) {
            members.push(id);
        }
    }
    basic(C, &json(&members), &args)
}

/// IsGroupMember: "True" or "False" (C#'s bool.ToString). An avatar or a group the bot does not
/// know is not a member, as with SecondBot.
pub async fn is_group_member(client: &Client, group: &str, who: &str) -> Answer {
    const C: &str = "IsGroupMember";
    let args = owned(&[group, who]);
    let Some(group) = key(group) else {
        return failure(C, "Invaild group UUID", &args);
    };
    let Some(id) = avatar(client, who).await else {
        return basic(C, "False", &[]);
    };
    match client.is_group_member(group, id).await {
        Ok(true) => basic(C, "True", &[]),
        Ok(false) | Err(ClientError::UnknownGroup) => basic(C, "False", &[]),
        Err(e) => failure(C, why(&e), &args),
    }
}

/// GroupInvite: invited with "everyone" and the role (none for "everyone"), as SecondBot's
/// Groups.Invite sends them.
pub async fn group_invite(client: &Client, group: &str, who: &str, role: &str) -> Answer {
    const C: &str = "GroupInvite";
    let args = owned(&[group, who, role]);
    let Some(group) = key(group) else {
        return failure(C, "Invaild group UUID", &args);
    };
    if !known_group(client, group) {
        return failure(C, "Unknown group", &args);
    }
    let Some(id) = avatar(client, who).await else {
        return failure(C, "avatar lookup", &args);
    };
    let mut roles = vec![Uuid::nil()];
    if role != "everyone" {
        let Some(role) = key(role) else {
            return failure(C, "Unable to process role UUID", &args);
        };
        roles.push(role);
    }
    match client.group_invite(group, id, &roles).await {
        Ok(()) => basic(C, "Invite sent", &args),
        Err(e) => failure(C, why(&e), &args),
    }
}

/// GroupEject: "Accepted" once the grid ejected the member.
pub async fn group_eject(client: &Client, group: &str, who: &str) -> Answer {
    const C: &str = "GroupEject";
    let args = owned(&[group, who]);
    let Some(group) = key(group) else {
        return failure(C, "Invaild group UUID", &args);
    };
    if !known_group(client, group) {
        return failure(C, "Unknown group", &args);
    }
    let Some(id) = avatar(client, who).await else {
        return failure(C, "Unknown avatar", &args);
    };
    match client.is_group_member(group, id).await {
        Ok(true) => {}
        Ok(false) => return failure(C, "Not in group", &args),
        Err(e) => return failure(C, why(&e), &args),
    }
    match client.group_eject(group, id).await {
        Ok(()) => basic(C, "Accepted", &[]),
        Err(ClientError::Refused(_)) => failure(C, "Missing group Eject power", &args),
        Err(e) => failure(C, why(&e), &args),
    }
}

/// GroupAddRole: the role for a member, an invitation with it for anyone else.
pub async fn group_add_role(client: &Client, group: &str, who: &str, role: &str) -> Answer {
    const C: &str = "GroupAddRole";
    let two = owned(&[group, who]);
    let args = owned(&[group, who, role]);
    let Some(group) = key(group) else {
        return failure(C, "Invaild group UUID", &two);
    };
    let Some(role) = key(role) else {
        return failure(C, "Invaild role UUID", &two);
    };
    if !known_group(client, group) {
        return failure(C, "Unknown group", &args);
    }
    let Some(id) = avatar(client, who).await else {
        return failure(C, "avatar lookup", &args);
    };
    match client.is_group_member(group, id).await {
        Ok(false) => match client.group_invite(group, id, &[Uuid::nil(), role]).await {
            Ok(()) => basic(C, "Invite sent", &[]),
            Err(e) => failure(C, why(&e), &args),
        },
        Ok(true) => match client.group_add_role(group, id, role).await {
            Ok(()) => basic(C, "Adding role", &[]),
            Err(e) => failure(C, why(&e), &args),
        },
        Err(e) => failure(C, why(&e), &args),
    }
}

/// GroupRemoveRole: the role taken from a member; "everyone" cannot be.
pub async fn group_remove_role(client: &Client, group: &str, who: &str, role: &str) -> Answer {
    const C: &str = "GroupRemoveRole";
    let args = owned(&[group, who, role]);
    let Some(group) = key(group) else {
        return failure(C, "Invaild group UUID", &args);
    };
    let Some(role) = key(role) else {
        return failure(C, "Invaild role UUID", &args);
    };
    if role.is_nil() {
        return failure(C, "The everyone role cannot be removed", &args);
    }
    if !known_group(client, group) {
        return failure(C, "Unknown group", &args);
    }
    let Some(id) = avatar(client, who).await else {
        return failure(C, "avatar lookup", &args);
    };
    match client.is_group_member(group, id).await {
        Ok(true) => {}
        Ok(false) => return failure(C, "Not in group", &args),
        Err(e) => return failure(C, why(&e), &args),
    }
    match client.group_remove_role(group, id, role).await {
        Ok(()) => basic(C, "Removing role", &args),
        Err(e) => failure(C, why(&e), &args),
    }
}

/// Groupnotice and GroupnoticeWithAttachment: "Sending notice" (with attachment).
pub async fn group_notice(
    client: &Client,
    group: &str,
    title: &str,
    message: &str,
    attachment: Option<&str>,
) -> Answer {
    let (c, args) = match attachment {
        Some(a) => (
            "GroupnoticeWithAttachment",
            owned(&[group, title, message, a]),
        ),
        None => ("Groupnotice", owned(&[group, title, message])),
    };
    let Some(id) = key(group) else {
        return failure(c, "Invaild group UUID", &args);
    };
    let item = match attachment {
        Some(a) => match key(a) {
            Some(item) => Some(item),
            None => return failure(c, "Invaild inventory UUID", &args),
        },
        None => None,
    };
    if !known_group(client, id) {
        return failure(c, "Unknown group", &args);
    }
    if title.trim().is_empty() {
        return failure(c, "Title empty", &args);
    }
    if message.trim().is_empty() {
        return failure(c, "Message empty", &args);
    }
    let done = if item.is_some() {
        "Sending notice with attachment"
    } else {
        "Sending notice"
    };
    match client.group_notice(id, title, message, item).await {
        Ok(()) => basic(c, done, &args),
        Err(e) => failure(c, why(&e), &args),
    }
}

/// GetGroupBanList: the keys of the banned avatars.
pub async fn get_group_ban_list(client: &Client, group: &str) -> Answer {
    const C: &str = "GetGroupBanList";
    let args = owned(&[group]);
    let Some(id) = key(group) else {
        return failure(C, "Invaild group UUID", &args);
    };
    if !known_group(client, id) {
        return failure(C, "Unknown group", &args);
    }
    match client.group_bans(id).await {
        Ok(bans) => {
            let ids: Vec<String> = bans.iter().map(ToString::to_string).collect();
            basic(C, &json(&ids), &args)
        }
        Err(ClientError::Timeout) => failure(C, "No reply from the grid [Retry later]", &args),
        Err(_) => failure(
            C,
            "Ban list request failed [Does the bot have the GroupBanAccess power?]",
            &args,
        ),
    }
}

/// GroupBan: ejected first, then banned ("true") or unbanned ("false").
pub async fn group_ban(client: &Client, group: &str, who: &str, state: &str) -> Answer {
    const C: &str = "GroupBan";
    let args = owned(&[group, who, state]);
    let Some(gid) = key(group) else {
        return failure(C, "Invaild group UUID", &args);
    };
    if !known_group(client, gid) {
        return failure(C, "Unknown group", &args);
    }
    let Some(id) = avatar(client, who).await else {
        return failure(C, "Unknown avatar", &args);
    };
    let ban = state != "false";
    if ban && client.is_group_member(gid, id).await == Ok(true) {
        let _ = client.group_eject(gid, id).await;
    }
    match client.group_ban(gid, &[id], ban).await {
        Ok(_) => {
            let action = if ban { "Ban" } else { "Unban" };
            basic(C, &format!("Accepted {action} for {id}"), &[])
        }
        Err(_) => failure(C, "Missing group GroupBanAccess power", &args),
    }
}

/// GetGroupTitles: `{role: {title, selected}}` of the bot's titles.
pub async fn get_group_titles(client: &Client, group: &str) -> Answer {
    const C: &str = "GetGroupTitles";
    let args = owned(&[group]);
    let Some(id) = key(group) else {
        return failure(C, "Invaild group UUID", &args);
    };
    if !known_group(client, id) {
        return failure(C, "Unknown group", &args);
    }
    #[derive(serde::Serialize)]
    struct Title {
        title: String,
        selected: bool,
    }
    match client.group_titles(id).await {
        Ok(titles) => {
            let map: BTreeMap<String, Title> = titles
                .into_iter()
                .map(|t| {
                    (
                        t.role.to_string(),
                        Title {
                            title: t.title,
                            selected: t.selected,
                        },
                    )
                })
                .collect();
            basic(C, &json(&map), &args)
        }
        Err(_) => failure(C, "No reply from the grid [Retry later]", &args),
    }
}

/// GroupActiveTitle: "Switching title".
pub async fn group_active_title(client: &Client, group: &str, role: &str) -> Answer {
    const C: &str = "GroupActiveTitle";
    let args = owned(&[group, role]);
    let Some(gid) = key(group) else {
        return failure(C, "Invaild group UUID", &args);
    };
    let Some(rid) = key(role) else {
        return failure(C, "Invaild role UUID", &args);
    };
    if !known_group(client, gid) {
        return failure(C, "Unknown group", &args);
    }
    match client.group_activate_title(gid, rid).await {
        Ok(()) => basic(C, "Switching title", &args),
        Err(e) => failure(C, why(&e), &args),
    }
}

/// GroupActiveGroup: "Switching active group".
pub async fn group_active_group(client: &Client, group: &str) -> Answer {
    const C: &str = "GroupActiveGroup";
    let args = owned(&[group]);
    let Some(gid) = key(group) else {
        return failure(C, "Invaild group UUID", &args);
    };
    if !known_group(client, gid) {
        return failure(C, "Unknown group", &args);
    }
    match client.group_activate(gid).await {
        Ok(()) => basic(C, "Switching active group", &args),
        Err(e) => failure(C, why(&e), &args),
    }
}

/// GroupJoin: a group with open enrollment.
pub async fn group_join(client: &Client, group: &str) -> Answer {
    const C: &str = "GroupJoin";
    let args = owned(&[group]);
    let Some(gid) = key(group) else {
        return failure(C, "Invaild group UUID", &args);
    };
    if known_group(client, gid) {
        return basic(C, "Already in group", &args);
    }
    match client.group_join(gid).await {
        Ok(true) => {
            let _ = client.reload_groups().await;
            basic(C, "Joined group", &args)
        }
        Ok(false) => failure(
            C,
            "Join failed (group is invite only or the fee could not be paid)",
            &args,
        ),
        Err(_) => failure(C, "No reply from the grid [Retry later]", &args),
    }
}

/// GroupCreate: a new group, the bot its founder. It costs L$100, so it is a Funds command (off
/// unless FERRO_ALLOW_FUNDS). SecondBot's checks, in its order; the balance is asked of the grid
/// (SecondBot reads the one it last heard, and asks for the next call).
pub async fn group_create(
    ctx: &super::Ctx,
    name: &str,
    charter: &str,
    open_enrollment: &str,
    membership_fee: &str,
    show_in_list: &str,
) -> Answer {
    const C: &str = "GroupCreate";
    /// What the grid charges for a new group.
    const PRICE: i32 = 100;
    let args = owned(&[name, charter, open_enrollment, membership_fee, show_in_list]);
    if !ctx.allow_funds {
        return failure(C, "Funds commands are disabled", &args);
    }
    // C#'s string lengths count UTF-16 units
    let length = |s: &str| s.encode_utf16().count();
    if !(4..=35).contains(&length(name)) {
        return failure(C, "Name must be 4 to 35 characters", &args);
    }
    if length(charter) > 511 {
        return failure(C, "Charter must be 511 characters or less", &args);
    }
    let Some(open) = super::cs_bool(open_enrollment) else {
        return failure(C, "Invaild openenrollment value", &args);
    };
    let Some(listed) = super::cs_bool(show_in_list) else {
        return failure(C, "Invaild showinlist value", &args);
    };
    let fee = match membership_fee.trim().parse::<i32>() {
        Ok(fee) if (0..=100_000).contains(&fee) => fee,
        _ => return failure(C, "Invaild membershipfee value (0 to 100000)", &args),
    };
    let client = &ctx.client;
    match client.balance().await {
        Ok(balance) if balance >= PRICE => {}
        Ok(_) => return failure(C, "Not enough funds (L$100 needed)", &args),
        Err(e) => {
            log::debug!("{C}: the balance: {e}");
            return failure(C, "No reply from the grid [Retry later]", &args);
        }
    }
    let group = ferro_client::groups::NewGroup {
        name: name.to_owned(),
        charter: charter.to_owned(),
        show_in_list: listed,
        insignia: Uuid::nil(),
        membership_fee: fee,
        open_enrollment: open,
        allow_publish: listed,
        mature_publish: false,
    };
    match client.create_group(group).await {
        Ok(created) if created.success => basic(C, &created.id.to_string(), &args),
        Ok(created) => failure(C, &format!("Create failed: {}", created.message), &args),
        Err(e) => {
            log::debug!("{C}: {e}");
            failure(C, "No reply from the grid [Retry later]", &args)
        }
    }
}

/// GroupLeave: the bot leaves, its roles and powers in the group with it.
pub async fn group_leave(client: &Client, group: &str) -> Answer {
    const C: &str = "GroupLeave";
    let args = owned(&[group]);
    let Some(gid) = key(group) else {
        return failure(C, "Invaild group UUID", &args);
    };
    if !known_group(client, gid) {
        return failure(C, "Unknown group", &args);
    }
    match client.group_leave(gid).await {
        Ok(true) => {
            let _ = client.reload_groups().await;
            basic(C, "Left group", &args)
        }
        Ok(false) => failure(C, "Leave failed", &args),
        Err(_) => failure(C, "No reply from the grid [Retry later]", &args),
    }
}

/// GetGroupMemberRoles: `{role: name}` of a member's roles.
pub async fn get_group_member_roles(client: &Client, group: &str, who: &str) -> Answer {
    const C: &str = "GetGroupMemberRoles";
    let args = owned(&[group, who]);
    let Some(gid) = key(group) else {
        return failure(C, "Invaild group UUID", &args);
    };
    if !known_group(client, gid) {
        return failure(C, "Unknown group", &args);
    }
    let Some(id) = avatar(client, who).await else {
        return failure(C, "avatar lookup", &args);
    };
    match client.is_group_member(gid, id).await {
        Ok(true) => {}
        Ok(false) => return failure(C, "Not in group", &args),
        Err(e) => return failure(C, why(&e), &args),
    }
    let (Ok(pairs), Ok(roles)) = (
        client.group_role_members(gid).await,
        client.group_roles(gid).await,
    ) else {
        return failure(C, "No reply from the grid [Retry later]", &args);
    };
    let map: BTreeMap<String, String> = pairs
        .iter()
        .filter(|(_, m)| *m == id)
        .map(|(r, _)| {
            let name = roles
                .iter()
                .find(|x| x.id == *r)
                .map(|x| x.name.clone())
                .unwrap_or_default();
            (r.to_string(), name)
        })
        .collect();
    basic(C, &json(&map), &args)
}

/// ForceLoadGroups: "Working on it".
pub async fn force_load_groups(client: &Client) -> Answer {
    let _ = client.reload_groups().await;
    basic("ForceLoadGroups", "Working on it", &[])
}
