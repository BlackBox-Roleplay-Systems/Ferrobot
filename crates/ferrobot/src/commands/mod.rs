//! SecondBot's commands, by name: the ones BB's SL group sync sends first (ROADMAP phase 4). Each
//! answers as SecondBot's does, the same reply texts and failures, so that its users can switch.
//! A deliberate difference is noted on the command.

mod animation;
mod avatars;
mod bot;
mod chat;
mod dialogs;
mod discord;
mod estate;
mod groups;
mod info;
mod inventory;
mod movement;
mod notecard;
mod objects;
mod outfit;
mod parcel;
pub mod reply;
mod world;

use crate::dialogs::Dialogs;
use crate::history::History;
use crate::services::homebound::Place;
use crate::store::{CommandRecord, Store};
use ferro_client::Client;
use reply::Answer;
use std::sync::Arc;
use uuid::Uuid;

/// C#'s bool.TryParse: "true" or "false", whatever the case.
pub fn cs_bool(s: &str) -> Option<bool> {
    s.trim().to_ascii_lowercase().parse().ok()
}

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
    /// Homebound, paused by a teleport command.
    pub homebound: Arc<crate::services::homebound::Homebound>,
    /// The Funds commands (FERRO_ALLOW_FUNDS).
    pub allow_funds: bool,
    /// For the replies that go out by HTTP (smart replies).
    pub http: reqwest::Client,
    /// The avatars whose objects may animate the bot.
    pub animators: Arc<crate::animators::Animators>,
    /// The Discord connection, when Discord is on.
    pub discord: Option<Arc<crate::discord::Discord>>,
    /// Who hears of the items that come into the inventory (SetInventoryUpdate).
    pub inventory_updates: Arc<crate::inventory_updates::InventoryUpdates>,
}

/// The commands that change things in world: under the rate limit.
const CHANGING: &[&str] = &[
    "SendItem",
    "SendFolder",
    "SendItemByPath",
    "SendFolderByPath",
    "SendItemDirect",
    "AsyncSendItem",
    "AsyncSendItemByPath",
    "NotecardSend",
    "NotecardDirectSend",
    "Teleport",
    "TeleportSLURL",
    "SendTeleportLure",
    "RequestTeleport",
    "FriendRequest",
    "FriendFullPerms",
    "PayAvatar",
    "PayObject",
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
    "SetParcelName",
    "SetParcelDesc",
    "SetParcelLandingZone",
    "SetParcelMusic",
    "SetParcelSale",
    "CancelParcelSale",
    "SetParcelSnapshot",
    "SetParcelAudioControls",
    "SetParcelFlag",
    "ParcelSetMedia",
    "ParcelEject",
    "ParcelFreeze",
    "ParcelBan",
    "ParcelUnBan",
    "ResetParcelAcccessLists",
    "ReturnAllObjectsOnParcel",
    "ReturnAllObjectsOnParcelByOwner",
    "ParcelDeedToGroup",
    "AbandonLand",
    "ParcelBuy",
    "EstateParcelReclaim",
    "EstateKickUser",
    "EstateTeleportHomeUser",
    "AddEstateManager",
    "RemoveEstateManager",
    "UpdateEstateBanlist",
    "SimRestart",
    "SimMessage",
    "SetEstateCovenant",
    "SetExtendedRegionInfo",
    "SetRegionFlags",
    "SetEstateConfig",
    "SetEstateTerrainRaw",
    "AddToAllowAnimations",
    "SetProfileAbout",
    "SetProfileImage",
    "RezObject",
    "RezObjectOnPos",
    "RezObjectParcelCenter",
    "AsyncRezObject",
    "AsyncRezObjectOnPos",
    "AsyncRezObjectParcelCenter",
    "ClickObject",
    "UnRezObject",
    "TransferInventoryToObject",
    "AsyncTransferInventoryToObject",
    "InventoryPurgeNotecards",
    "FetchNextNotecard",
    "Attach",
    "Detach",
    "SwapOutfit",
    "SwapOutfitByPath",
    "Outfit",
    "Discord_AddRole",
    "Discord_BanMember",
    "Discord_BulkClear_Messages",
    "Discord_Dm_Member",
    "Discord_MessageChannel",
    "Discord_MuteMember",
    "Discord_Role_UpdatePerms",
    "Discord_RoleCreate",
    "Discord_RoleRemove",
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
    ("SendItemByPath", 2),
    ("SendFolderByPath", 2),
    ("SendItemDirect", 4),
    ("AsyncSendItem", 3),
    ("AsyncSendItemByPath", 3),
    ("AsyncInventoryContents", 2),
    ("NearmeWithDetails", 0),
    ("FriendsGetLocation", 1),
    ("GetSimGlobalPos", 1),
    ("GetSimTexture", 1),
    ("GetMuteList", 1),
    ("SetAway", 1),
    ("GetCurrentParcelId", 0),
    ("GetCurrentParcelUUID", 0),
    ("GetCurrentParcelSize", 0),
    ("GetParcelDesc", 0),
    ("GetParcelFlags", 0),
    ("GetParcelPrimCounts", 0),
    ("GetParcelSaleDetails", 0),
    ("GetParcelTraffic", 0),
    ("ParcelName", 0),
    ("GetCurrentRegionType", 0),
    ("UpdateListOfParcels", 0),
    ("GetListOfParcels", 0),
    ("GetParcelListSnapshots", 0),
    ("GetSimParcelLayers", 0),
    ("QueryLandSaleData", 5),
    ("SetParcelName", 1),
    ("SetParcelDesc", 1),
    ("SetParcelLandingZone", 3),
    ("SetParcelMusic", 1),
    ("SetParcelSale", 2),
    ("CancelParcelSale", 0),
    ("SetParcelSnapshot", 1),
    ("SetParcelAudioControls", 2),
    ("SetParcelFlag", 1),
    ("ParcelSetMedia", 1),
    ("ParcelEject", 1),
    ("ParcelFreeze", 2),
    ("ParcelBan", 1),
    ("ParcelUnBan", 1),
    ("GetParcelBanlist", 0),
    ("ResetParcelAcccessLists", 0),
    ("ReturnAllObjectsOnParcel", 0),
    ("ReturnAllObjectsOnParcelByOwner", 1),
    ("ParcelDeedToGroup", 1),
    ("AbandonLand", 0),
    ("ParcelBuy", 1),
    ("EstateParcelReclaim", 0),
    ("EstateKickUser", 1),
    ("EstateTeleportHomeUser", 1),
    ("AddEstateManager", 2),
    ("RemoveEstateManager", 2),
    ("UpdateEstateBanlist", 3),
    ("GetEstateBanList", 0),
    ("SimRestart", 2),
    ("SimMessage", 1),
    ("SetEstateCovenant", 1),
    ("GetEstateCovenant", 0),
    ("SetExtendedRegionInfo", 12),
    ("SetRegionFlags", 10),
    ("SetEstateConfig", 10),
    ("SetEstateTerrainRaw", 1),
    ("AddToAllowAnimations", 1),
    ("RezObject", 1),
    ("RezObjectOnPos", 4),
    ("RezObjectParcelCenter", 2),
    ("AsyncRezObject", 2),
    ("AsyncRezObjectOnPos", 5),
    ("AsyncRezObjectParcelCenter", 3),
    ("ClickObject", 1),
    ("UnRezObject", 1),
    ("GetObjectsInParcel", 0),
    ("TransferInventoryToObject", 3),
    ("AsyncTransferInventoryToObject", 4),
    ("Attach", 1),
    ("Detach", 1),
    ("SwapOutfit", 1),
    ("SwapOutfitByPath", 1),
    ("Outfit", 1),
    ("Discord_AddRole", 3),
    ("Discord_BanMember", 3),
    ("Discord_GetMemberRoles", 2),
    ("Discord_BulkClear_Messages", 2),
    ("Discord_Dm_Member", 3),
    ("Discord_MembersList", 1),
    ("Discord_MessageChannel", 4),
    ("Discord_MuteMember", 3),
    ("Discord_Role_GetSettings", 2),
    ("Discord_Role_UpdatePerms", 3),
    ("Discord_RoleCreate", 2),
    ("Discord_RoleList", 1),
    ("Discord_RoleRemove", 2),
    ("Discord_TextChannels_List", 1),
    ("Discord_VoiceJoin", 1),
    ("Discord_VoiceLeave", 0),
    ("GetProfileAbout", 1),
    ("GetProfileImage", 1),
    ("SetProfileAbout", 1),
    ("SetProfileImage", 1),
    ("PlayGesture", 1),
    ("PlayGestureDirect", 1),
    ("ResetAnimations", 0),
    ("PointAt", 1),
    ("GetRealUUID", 1),
    ("CreateInventoryFolder", 2),
    ("RenameInventory", 2),
    ("DeleteInventoryItem", 1),
    ("DeleteInventoryFolder", 1),
    ("MoveInventoryItem", 2),
    ("MoveInventoryFolder", 2),
    ("InventoryPurgeNotecards", 0),
    ("InventoryContentsCatched", 0),
    ("InventoryPreloadCount", 0),
    ("InventoryPreloadDone", 0),
    ("SetInventoryUpdate", 2),
    ("FetchNextNotecard", 2),
    ("ListSculptys", 0),
    ("SetPermFlag", 4),
    ("Fly", 1),
    ("SetAlwaysRun", 1),
    ("Sit", 1),
    ("Stand", 0),
    ("AutoPilot", 3),
    ("AutoPilotStop", 0),
    ("RotateTo", 1),
    ("RotateToFace", 1),
    ("RotateToFaceVector", 1),
    ("HoverHeight", 1),
    ("Teleport", 4),
    ("TeleportSLURL", 1),
    ("SendTeleportLure", 1),
    ("RequestTeleport", 1),
    ("Key2Name", 1),
    ("GetDisplayName", 1),
    ("Nearme", 0),
    ("Friendslist", 0),
    ("FriendRequest", 2),
    ("FriendFullPerms", 2),
    ("Balance", 0),
    ("PayAvatar", 2),
    ("PayObject", 3),
    ("NotecardAdd", 2),
    ("NotecardRead", 2),
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
        "SendItemByPath" => inventory::send_item_by_path(ctx, a(0), a(1)).await,
        "SendFolderByPath" => inventory::send_folder_by_path(ctx, a(0), a(1)).await,
        "SendItemDirect" => inventory::send_item_direct(ctx, a(0), a(1), a(2), a(3)).await,
        "AsyncSendItem" => inventory::async_send_item(ctx, a(0), a(1), a(2)).await,
        "AsyncSendItemByPath" => inventory::async_send_item_by_path(ctx, a(0), a(1), a(2)).await,
        "AsyncInventoryContents" => inventory::async_contents(ctx, a(0), a(1)).await,
        "NearmeWithDetails" => avatars::nearme_with_details(client).await,
        "FriendsGetLocation" => world::friend_location(ctx, a(0)).await,
        "GetSimGlobalPos" => world::sim_global_pos(ctx, a(0)).await,
        "GetSimTexture" => world::sim_texture(ctx, a(0)).await,
        "GetMuteList" => bot::mute_list(ctx, a(0)).await,
        "SetAway" => bot::set_away(ctx, a(0)).await,
        "GetCurrentParcelId" => parcel::current_id(ctx).await,
        "GetCurrentParcelUUID" => parcel::current_uuid(ctx).await,
        "GetCurrentParcelSize" => parcel::current_size(ctx).await,
        "GetParcelDesc" => parcel::desc(ctx).await,
        "GetParcelFlags" => parcel::flags(ctx).await,
        "GetParcelPrimCounts" => parcel::prim_counts(ctx).await,
        "GetParcelSaleDetails" => parcel::sale_details(ctx).await,
        "GetParcelTraffic" => parcel::traffic(ctx).await,
        "ParcelName" => parcel::parcel_name(ctx).await,
        "GetCurrentRegionType" => parcel::region_type(ctx),
        "UpdateListOfParcels" => parcel::update_list(ctx).await,
        "GetListOfParcels" => parcel::list(ctx),
        "GetParcelListSnapshots" => parcel::snapshots(ctx),
        "GetSimParcelLayers" => parcel::layers(ctx),
        "QueryLandSaleData" => parcel::land_sale(ctx, a(0), a(1), a(2), a(3), a(4)).await,
        "SetParcelName" => parcel::set_name(ctx, a(0)).await,
        "SetParcelDesc" => parcel::set_desc(ctx, a(0)).await,
        "SetParcelLandingZone" => parcel::set_landing(ctx, a(0), a(1), a(2)).await,
        "SetParcelMusic" => parcel::set_music(ctx, a(0)).await,
        "SetParcelSale" => parcel::set_sale(ctx, a(0), a(1)).await,
        "CancelParcelSale" => parcel::cancel_sale(ctx).await,
        "SetParcelSnapshot" => parcel::set_snapshot(ctx, a(0)).await,
        "SetParcelAudioControls" => parcel::set_audio(ctx, a(0), a(1)).await,
        "SetParcelFlag" => parcel::set_flags(ctx, a(0)).await,
        "ParcelSetMedia" => parcel::set_media(ctx, a(0)).await,
        "ParcelEject" => parcel::eject(ctx, a(0)).await,
        "ParcelFreeze" => parcel::freeze(ctx, a(0), a(1)).await,
        "ParcelBan" => parcel::ban(ctx, a(0)).await,
        "ParcelUnBan" => parcel::unban(ctx, a(0)).await,
        "GetParcelBanlist" => parcel::get_ban_list(ctx).await,
        "ResetParcelAcccessLists" => parcel::reset_access_lists(ctx).await,
        "ReturnAllObjectsOnParcel" => parcel::return_all(ctx).await,
        "ReturnAllObjectsOnParcelByOwner" => parcel::return_by_owner(ctx, a(0)).await,
        "ParcelDeedToGroup" => parcel::deed_to_group(ctx, a(0)).await,
        "AbandonLand" => parcel::abandon(ctx).await,
        "ParcelBuy" => parcel::buy(ctx, a(0)).await,
        "EstateParcelReclaim" => parcel::reclaim(ctx).await,
        "EstateKickUser" => estate::kick(ctx, a(0)).await,
        "EstateTeleportHomeUser" => estate::teleport_home(ctx, a(0)).await,
        "AddEstateManager" => estate::manager_change(ctx, true, a(0), a(1)).await,
        "RemoveEstateManager" => estate::manager_change(ctx, false, a(0), a(1)).await,
        "UpdateEstateBanlist" => estate::update_ban_list(ctx, a(0), a(1), a(2)).await,
        "GetEstateBanList" => estate::ban_list(ctx).await,
        "SimRestart" => estate::restart(ctx, a(0), a(1)).await,
        "SimMessage" => estate::sim_message(ctx, a(0)).await,
        "SetEstateCovenant" => estate::set_covenant(ctx, a(0)).await,
        "GetEstateCovenant" => estate::get_covenant(ctx).await,
        "SetExtendedRegionInfo" => {
            let values = std::array::from_fn(&a);
            estate::set_region_info(
                ctx,
                "SetExtendedRegionInfo",
                estate::RegionValues { values },
            )
            .await
        }
        // SecondBot's SetRegionFlags calls SetExtendedRegionInfo: its answer is named so
        "SetRegionFlags" => {
            let values = [
                a(0),
                a(1),
                "false",
                a(2),
                a(3),
                a(6),
                a(7),
                a(8),
                a(9),
                a(4),
                a(5),
                "false",
            ];
            estate::set_region_info(
                ctx,
                "SetExtendedRegionInfo",
                estate::RegionValues { values },
            )
            .await
        }
        "RezObject" => objects::rez_object(ctx, a(0)).await,
        "RezObjectOnPos" => objects::rez_object_on_pos(ctx, a(0), a(1), a(2), a(3)).await,
        "RezObjectParcelCenter" => objects::rez_object_parcel_center(ctx, a(0), a(1)).await,
        "AsyncRezObject" => objects::async_rez_object(ctx, a(0), a(1)).await,
        "AsyncRezObjectOnPos" => {
            objects::async_rez_object_on_pos(ctx, a(0), a(1), a(2), a(3), a(4)).await
        }
        "AsyncRezObjectParcelCenter" => {
            objects::async_rez_object_parcel_center(ctx, a(0), a(1), a(2)).await
        }
        "ClickObject" => objects::click(ctx, a(0)).await,
        "UnRezObject" => objects::unrez(ctx, a(0)).await,
        "GetObjectsInParcel" => objects::objects_in_parcel(ctx).await,
        "TransferInventoryToObject" => objects::transfer_to_object(ctx, a(0), a(1), a(2)).await,
        "AsyncTransferInventoryToObject" => {
            objects::async_transfer_to_object(ctx, a(0), a(1), a(2), a(3)).await
        }
        "Attach" => outfit::attach(ctx, a(0)).await,
        "Detach" => outfit::detach(ctx, a(0)).await,
        "SwapOutfit" => outfit::swap_outfit(ctx, a(0)).await,
        "SwapOutfitByPath" => outfit::swap_outfit_by_path(ctx, a(0)).await,
        "Outfit" => outfit::outfit(ctx, a(0)).await,
        "Discord_AddRole" => discord::add_role(ctx, a(0), a(1), a(2)).await,
        "Discord_BanMember" => discord::ban_member(ctx, a(0), a(1), a(2)).await,
        "Discord_GetMemberRoles" => discord::member_roles(ctx, a(0), a(1)).await,
        "Discord_BulkClear_Messages" => discord::bulk_clear(ctx, a(0), a(1)).await,
        "Discord_Dm_Member" => discord::dm_member(ctx, a(0), a(1), a(2)).await,
        "Discord_MembersList" => discord::members_list(ctx, a(0)).await,
        "Discord_MessageChannel" => discord::message_channel(ctx, a(0), a(1), a(2), a(3)).await,
        "Discord_MuteMember" => discord::mute_member(ctx, a(0), a(1), a(2)).await,
        "Discord_Role_GetSettings" => discord::role_settings(ctx, a(0), a(1)).await,
        "Discord_Role_UpdatePerms" => discord::role_update_perms(ctx, a(0), a(1), a(2)).await,
        "Discord_RoleCreate" => discord::role_create(ctx, a(0), a(1)).await,
        "Discord_RoleList" => discord::role_list(ctx, a(0)).await,
        "Discord_RoleRemove" => discord::role_remove(ctx, a(0), a(1)).await,
        "Discord_TextChannels_List" => discord::text_channels(ctx, a(0)).await,
        "Discord_VoiceJoin" => discord::voice_join(ctx, a(0)),
        "Discord_VoiceLeave" => discord::voice_leave(),
        "GetProfileAbout" => avatars::profile_about(client, a(0)).await,
        "GetProfileImage" => avatars::profile_image(client, a(0)).await,
        "SetProfileAbout" => avatars::set_profile_about(client, a(0)).await,
        "SetProfileImage" => avatars::set_profile_image(client, a(0)).await,
        "AddToAllowAnimations" => animation::allow_animations(ctx, a(0)).await,
        "PlayGesture" => animation::play_gesture(ctx, a(0)).await,
        "PlayGestureDirect" => animation::play_gesture_direct(ctx, a(0)).await,
        "ResetAnimations" => animation::reset_animations(ctx).await,
        "PointAt" => animation::point_at(ctx, a(0)).await,
        "SetEstateConfig" => estate::set_estate_config(ctx, std::array::from_fn(&a)).await,
        "SetEstateTerrainRaw" => estate::set_terrain_raw(ctx, a(0)).await,
        "GetRealUUID" => inventory::real_uuid(ctx, a(0)).await,
        "CreateInventoryFolder" => inventory::create_folder(ctx, a(0), a(1)).await,
        "RenameInventory" => inventory::rename(ctx, a(0), a(1)).await,
        "DeleteInventoryItem" => inventory::delete_item(ctx, a(0)).await,
        "DeleteInventoryFolder" => inventory::delete_folder(ctx, a(0)).await,
        "MoveInventoryItem" => inventory::move_item(ctx, a(0), a(1)).await,
        "MoveInventoryFolder" => inventory::move_folder(ctx, a(0), a(1)).await,
        "InventoryPurgeNotecards" => inventory::purge_notecards(ctx).await,
        "InventoryContentsCatched" => inventory::contents_catched(ctx).await,
        "InventoryPreloadCount" => inventory::preload_count(ctx),
        "InventoryPreloadDone" => inventory::preload_done(),
        "SetInventoryUpdate" => inventory::set_inventory_update(ctx, a(0), a(1)),
        "FetchNextNotecard" => notecard::fetch_next(ctx, a(0), a(1)).await,
        "ListSculptys" => objects::list_sculptys(ctx).await,
        "SetPermFlag" => bot::set_perm_flag(ctx, a(0), a(1), a(2), a(3)).await,
        "Fly" => movement::fly(ctx, a(0)).await,
        "SetAlwaysRun" => movement::always_run(ctx, a(0)).await,
        "Sit" => movement::sit(ctx, a(0)).await,
        "Stand" => movement::stand(ctx).await,
        "AutoPilot" => movement::autopilot(ctx, a(0), a(1), a(2)).await,
        "AutoPilotStop" => movement::autopilot_stop(ctx).await,
        "RotateTo" => movement::rotate_to(ctx, a(0)).await,
        "RotateToFace" => movement::rotate_to_face(ctx, a(0)).await,
        "RotateToFaceVector" => movement::rotate_to_face_vector(ctx, a(0)).await,
        "HoverHeight" => movement::hover_height(ctx, a(0)).await,
        "Teleport" => world::teleport_to(ctx, a(0), a(1), a(2), a(3)).await,
        "TeleportSLURL" => world::teleport_slurl(ctx, a(0)).await,
        "SendTeleportLure" => world::send_teleport_lure(ctx, a(0)).await,
        "RequestTeleport" => world::request_teleport(ctx, a(0)).await,
        "Key2Name" => world::key2name(ctx, a(0)).await,
        "GetDisplayName" => world::display_name(ctx, a(0)).await,
        "Nearme" => world::nearme(ctx).await,
        "Friendslist" => world::friends_list(ctx).await,
        "FriendRequest" => world::friend_request(ctx, a(0), a(1)).await,
        "FriendFullPerms" => world::friend_full_perms(ctx, a(0), a(1)).await,
        "Balance" => world::balance(ctx).await,
        "PayAvatar" => world::pay_avatar(ctx, a(0), a(1)).await,
        "PayObject" => world::pay_object(ctx, a(0), a(1), a(2)).await,
        "NotecardAdd" => notecard::add(ctx, a(0), a(1)),
        "NotecardRead" => notecard::read(ctx, a(0), a(1)).await,
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
            homebound: Arc::default(),
            allow_funds: false,
            http: reqwest::Client::new(),
            animators: Arc::default(),
            discord: None,
            inventory_updates: Arc::default(),
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
