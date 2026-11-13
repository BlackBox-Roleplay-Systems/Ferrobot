//! SecondBot's commands, by name: the ones BB's SL group sync sends first (ROADMAP phase 4). Each
//! answers as SecondBot's does, the same reply texts and failures, so that its users can switch.
//! A deliberate difference is noted on the command.

mod avatars;
mod bot;
mod chat;
mod dialogs;
mod groups;
mod info;
mod inventory;
mod notecard;
pub mod reply;

use crate::dialogs::Dialogs;
use crate::history::History;
use crate::services::homebound::Place;
use crate::store::{CommandRecord, Store};
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
    pub store: Arc<Store>,
    /// Homebound's home, for GoHome.
    pub home: Option<Place>,
    /// Stops the bot (Logoff).
    pub stop: Arc<tokio::sync::watch::Sender<bool>>,
}

/// The commands that change things in world: under the rate limit.
const CHANGING: &[&str] = &[
    "SendItem",
    "SendFolder",
    "NotecardSend",
    "NotecardDirectSend",
    "GroupInvite",
    "GroupEject",
    "GroupAddRole",
    "GroupRemoveRole",
    "GroupBan",
    "Groupnotice",
    "GroupnoticeWithAttachment",
    "GroupJoin",
    "GroupLeave",
    "Say",
    "Shout",
    "Whisper",
    "IM",
    "Groupchat",
];

/// How long a command that changes things may wait for the rate limit.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(60);

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
    ("Hello", 0),
    ("Version", 0),
    ("UnixTimeNow", 0),
    ("GetPosition", 0),
    ("GoHome", 0),
    ("Logoff", 0),
    ("GetLastCommands", 0),
    ("ReadKeyValue", 1),
    ("SetKeyValue", 2),
    ("ClearKeyValue", 1),
    ("BotStatus", 0),
    ("RebakeAppearance", 0),
    ("InventoryFolders", 0),
    ("InventoryFoldersLimited", 1),
    ("InventoryContents", 1),
    ("SendItem", 2),
    ("SendFolder", 2),
    ("GetRealUUID", 1),
    ("CreateInventoryFolder", 2),
    ("RenameInventory", 2),
    ("DeleteInventoryItem", 1),
    ("DeleteInventoryFolder", 1),
    ("MoveInventoryItem", 2),
    ("MoveInventoryFolder", 2),
    ("NotecardAdd", 2),
    ("NotecardClear", 1),
    ("NotecardSend", 3),
    ("NotecardDirectSend", 3),
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
    if CHANGING.contains(&command) && !ctx.store.admit(PATIENCE).await {
        log::warn!("{command}: too many commands, refused");
        let answer = reply::failure(command, "Too many commands [Retry later]", args);
        return (true, answer);
    }
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
        "Hello" => info::hello(),
        "Version" => info::version(),
        "UnixTimeNow" => info::unix_time_now(),
        "GetPosition" => info::get_position(client),
        "GoHome" => bot::go_home(ctx),
        "Logoff" => bot::logoff(ctx),
        "GetLastCommands" => bot::get_last_commands(ctx),
        "ReadKeyValue" => bot::read_key_value(ctx, a(0)),
        "SetKeyValue" => bot::set_key_value(ctx, a(0), a(1)),
        "ClearKeyValue" => bot::clear_key_value(ctx, a(0)),
        "BotStatus" => bot::bot_status(ctx),
        "RebakeAppearance" => bot::rebake(ctx).await,
        "InventoryFolders" => inventory::folders(ctx).await,
        "InventoryFoldersLimited" => inventory::folders_limited(ctx, a(0)).await,
        "InventoryContents" => inventory::contents(ctx, a(0)).await,
        "SendItem" => inventory::send_item(ctx, a(0), a(1)).await,
        "SendFolder" => inventory::send_folder(ctx, a(0), a(1)).await,
        "GetRealUUID" => inventory::real_uuid(ctx, a(0)).await,
        "CreateInventoryFolder" => inventory::create_folder(ctx, a(0), a(1)).await,
        "RenameInventory" => inventory::rename(ctx, a(0), a(1)).await,
        "DeleteInventoryItem" => inventory::delete_item(ctx, a(0)).await,
        "DeleteInventoryFolder" => inventory::delete_folder(ctx, a(0)).await,
        "MoveInventoryItem" => inventory::move_item(ctx, a(0), a(1)).await,
        "MoveInventoryFolder" => inventory::move_folder(ctx, a(0), a(1)).await,
        "NotecardAdd" => notecard::add(ctx, a(0), a(1)),
        "NotecardClear" => notecard::clear(ctx, a(0)),
        "NotecardSend" => notecard::send(ctx, a(0), a(1), a(2)).await,
        "NotecardDirectSend" => notecard::direct_send(ctx, a(0), a(1), a(2)).await,
        "DialogRelayAvatarTarget" => dialogs::relay_avatar(ctx, a(0)).await,
        "DialogRelayChatTarget" => dialogs::relay_chat(ctx, a(0)),
        "DialogRelayHttpTarget" => dialogs::relay_http(ctx, a(0)),
        "DialogResponce" => dialogs::respond(ctx, a(0), a(1)).await,
        _ => return (false, "Unknown command".into()),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
    ctx.store.record(CommandRecord {
        Command: command.to_owned(),
        Args: args.to_vec(),
        Unixtime: now,
        status: answer.contains(r#""status":true"#),
        results: None,
    });
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
            store: Arc::new(Store::new(0)),
            home: None,
            stop: Arc::new(tokio::sync::watch::channel(false).0),
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
