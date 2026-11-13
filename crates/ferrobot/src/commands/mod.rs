//! SecondBot's commands, by name: the ones BB's SL group sync sends first (ROADMAP phase 4). Each
//! answers as SecondBot's does, the same reply texts and failures, so that its users can switch.
//! A deliberate difference is noted on the command.

mod avatars;
mod groups;
mod info;
pub mod reply;

use ferro_client::Client;
use reply::Answer;
use uuid::Uuid;

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
];

/// What CommandsService.RunCommand answers: whether the command ran, and its answer (or why not).
pub async fn run(client: &Client, name: &str, args: &[String]) -> (bool, String) {
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
