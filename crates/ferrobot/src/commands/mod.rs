//! SecondBot's commands, by name: the ones BB's SL group sync sends first (ROADMAP phase 4). Each
//! answers as SecondBot's does, the same reply texts and failures, so that its users can switch.
//! A deliberate difference is noted on the command.

mod avatars;
mod chat;
mod dialogs;
mod groups;
mod info;
pub mod reply;

use crate::dialogs::Dialogs;
use crate::history::History;
use ferro_client::Client;
use reply::Answer;
use std::sync::Arc;
use uuid::Uuid;

/// What the commands work with.
#[derive(Clone)]
pub struct Ctx {
    pub client: Client,
    pub history: Arc<History>,
    pub dialogs: Arc<Dialogs>,
}

/// The commands, with their number of arguments (SecondBot checks the count first).
const COMMANDS: &[(&str, usize)] = &[
    ("SimName", 0),
    ("Name", 0),
    ("Name2Key", 1),
    ("GetGroupList", 0),
    ("GetGroupRoles", 1),
    ("GetGroupMembers", 1),
    ("GetGroupRoleMembers", 2),
    ("IsGroupMember", 2),
    ("GroupInvite", 3),
    ("GroupEject", 2),
    ("GroupAddRole", 3),
    ("GroupRemoveRole", 3),
    ("LocalChatHistory", 0),
    ("Say", 2),
    ("Shout", 2),
    ("Whisper", 2),
    ("IM", 2),
    ("chatwindows", 0),
    ("listwithunread", 0),
    ("haveunreadims", 0),
    ("getimchat", 1),
    ("GroupchatListAllUnreadGroups", 0),
    ("GroupchatGroupHasUnread", 1),
    ("GroupchatAnyUnread", 0),
    ("GroupchatClearAll", 0),
    ("GroupchatHistory", 1),
    ("Groupchat", 2),
    ("Groupnotice", 3),
    ("GroupnoticeWithAttachment", 4),
    ("GetGroupBanList", 1),
    ("GroupBan", 3),
    ("GetGroupTitles", 1),
    ("GroupActiveTitle", 2),
    ("GroupActiveGroup", 1),
    ("GroupJoin", 1),
    ("GroupLeave", 1),
    ("GetGroupMemberRoles", 2),
    ("ForceLoadGroups", 0),
    ("DialogRelayAvatarTarget", 1),
    ("DialogRelayChatTarget", 1),
    ("DialogRelayHttpTarget", 1),
    ("DialogResponce", 2),
];

/// What CommandsService.RunCommand answers: whether the command ran, and its answer (or why not).
pub async fn run(ctx: &Ctx, name: &str, args: &[String]) -> (bool, String) {
    let client = &ctx.client;
    // command names do not depend on case
    let Some(&(command, expected)) = COMMANDS.iter().find(|(c, _)| c.eq_ignore_ascii_case(name))
    else {
        return (false, "Unknown command".into());
    };
    if args.len() != expected {
        return (
            false,
            format!(
                "incorrect number of args expected: {expected} but got: {}",
                args.len()
            ),
        );
    }
    let a = |i: usize| args[i].as_str();
    let answer: Answer = match command {
        "SimName" => info::sim_name(client),
        "Name" => info::name(client),
        "Name2Key" => avatars::name2key(client, a(0)).await,
        "GetGroupList" => groups::get_group_list(client),
        "GetGroupRoles" => groups::get_group_roles(client, a(0)).await,
        "GetGroupMembers" => groups::get_group_members(client, a(0)).await,
        "GetGroupRoleMembers" => groups::get_group_role_members(client, a(0), a(1)).await,
        "IsGroupMember" => groups::is_group_member(client, a(0), a(1)).await,
        "GroupInvite" => groups::group_invite(client, a(0), a(1), a(2)).await,
        "GroupEject" => groups::group_eject(client, a(0), a(1)).await,
        "GroupAddRole" => groups::group_add_role(client, a(0), a(1), a(2)).await,
        "GroupRemoveRole" => groups::group_remove_role(client, a(0), a(1), a(2)).await,
        "LocalChatHistory" => chat::local_chat_history(ctx),
        "Say" | "Shout" | "Whisper" => chat::speak(ctx, command, a(0), a(1)).await,
        "IM" => chat::im(ctx, a(0), a(1)).await,
        "chatwindows" => chat::chat_windows(ctx),
        "listwithunread" => chat::list_with_unread(ctx),
        "haveunreadims" => chat::have_unread_ims(ctx),
        "getimchat" => chat::get_im_chat(ctx, a(0)),
        "GroupchatListAllUnreadGroups" => chat::group_chat_unread_groups(ctx),
        "GroupchatGroupHasUnread" => chat::group_chat_has_unread(ctx, a(0)),
        "GroupchatAnyUnread" => chat::group_chat_any_unread(ctx),
        "GroupchatClearAll" => chat::group_chat_clear_all(ctx),
        "GroupchatHistory" => chat::group_chat_history(ctx, a(0)),
        "Groupchat" => chat::group_chat(ctx, a(0), a(1)).await,
        "Groupnotice" => groups::group_notice(client, a(0), a(1), a(2), None).await,
        "GroupnoticeWithAttachment" => {
            groups::group_notice(client, a(0), a(1), a(2), Some(a(3))).await
        }
        "GetGroupBanList" => groups::get_group_ban_list(client, a(0)).await,
        "GroupBan" => groups::group_ban(client, a(0), a(1), a(2)).await,
        "GetGroupTitles" => groups::get_group_titles(client, a(0)).await,
        "GroupActiveTitle" => groups::group_active_title(client, a(0), a(1)).await,
        "GroupActiveGroup" => groups::group_active_group(client, a(0)).await,
        "GroupJoin" => groups::group_join(client, a(0)).await,
        "GroupLeave" => groups::group_leave(client, a(0)).await,
        "GetGroupMemberRoles" => groups::get_group_member_roles(client, a(0), a(1)).await,
        "ForceLoadGroups" => groups::force_load_groups(client).await,
        "DialogRelayAvatarTarget" => dialogs::relay_avatar(ctx, a(0)).await,
        "DialogRelayChatTarget" => dialogs::relay_chat(ctx, a(0)),
        "DialogRelayHttpTarget" => dialogs::relay_http(ctx, a(0)),
        "DialogResponce" => dialogs::respond(ctx, a(0), a(1)).await,
        _ => return (false, "Unknown command".into()),
    };
    (true, answer)
}

/// An avatar given by key or by name (SecondBot's ProcessAvatar); `None` when the name is not
/// found.
async fn avatar(client: &Client, avatar: &str) -> Option<Uuid> {
    if let Ok(id) = Uuid::parse_str(avatar.trim()) {
        return Some(id);
    }
    match client.find_avatar(avatar).await {
        Ok(found) => found,
        Err(e) => {
            log::debug!("avatar lookup: {e}");
            None
        }
    }
}

/// A group or role key as SecondBot's UUID.TryParse reads it.
fn key(text: &str) -> Option<Uuid> {
    Uuid::parse_str(text.trim()).ok()
}

/// The arguments as owned strings, for the answers.
fn owned(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unknown_commands_and_argument_counts() {
        let http = reqwest::Client::new();
        let (client, _network) = Client::new(http);
        let client = Ctx {
            client,
            history: Arc::default(),
            dialogs: Arc::default(),
        };
        assert_eq!(
            run(&client, "NoSuchCommand", &[]).await,
            (false, "Unknown command".to_owned())
        );
        assert_eq!(
            run(&client, "groupinvite", &["a".into()]).await,
            (
                false,
                "incorrect number of args expected: 3 but got: 1".to_owned()
            )
        );
        // names do not depend on case, and the answer names the command as SecondBot spells it
        let (ok, answer) = run(&client, "simname", &[]).await;
        assert!(ok);
        assert!(answer.starts_with(r#"{"command":"SimName""#), "{answer}");
    }
}
