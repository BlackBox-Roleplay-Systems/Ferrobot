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
