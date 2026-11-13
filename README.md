# Ferrobot

A Second Life bot written in Rust, in the spirit of [SecondBot](https://github.com/Madpeterz/SecondBot) (.NET), run as a Docker container.

What it does so far: logs in, arrives in its region, stays connected (pings, agent updates, the event queue), logs chat and IMs it receives, logs out cleanly on `docker stop`, and logs in again by itself when the connection is lost.

## Settings

All settings come from environment variables.

| Variable | Default | What it is |
|---|---|---|
| `FERRO_USERNAME` | (required) | The bot's account: `First Last` or a username |
| `FERRO_PASSWORD` | (required) | Its password |
| `FERRO_LOGIN_URI` | Second Life | Another grid's login address (`secondlife`: Second Life) |
| `FERRO_START` | `last` | `last`, `home`, or `Region Name/x/y/z` |
| `FERRO_RELOG` | `true` | Log in again after the connection is lost |
| `FERRO_RELOG_DELAY` | `60` | Seconds to wait before logging in again |
| `FERRO_REGION_TIMEOUT` | `100` | Seconds without a packet from the region before the connection counts as lost, as the viewer. For tests: keep the default on a grid |
| `FERRO_EVENT_QUEUE_ERRORS` | `15` | Quick event queue failures in a row (1 s + 3 s more after each) before the queue counts as lost and the bot logs in again, as the viewer. For tests: keep the default on a grid |
| `FERRO_MASTERS` | (none) | Avatars the bot obeys, comma-separated: `First Last` or usernames |
| `FERRO_SHARED_SECRET` | (none) | The secret that signs the HTTP API's commands. Without it, the API is off |
| `FERRO_HTTP` | `true` | SecondBot's HTTP API (`POST /api/Run`, `POST /json`) |
| `FERRO_HTTP_PORT` | `80` | Its port (0: any free port, named in the log) |
| `FERRO_HTTP_BIND` | `0.0.0.0` | The address it listens on: `127.0.0.1` behind a tunnel on the same host |
| `FERRO_TIME_WINDOW` | `35` | Seconds a signed command stays valid |
| `FERRO_IM_COMMANDS` | `true` | Commands by IM: from masters, and signed ones from objects |
| `FERRO_ENFORCE_TIME_WINDOW` | `false` | Commands by IM carry their time and must be recent (the HTTP API's always do) |
| `FERRO_OBJECT_MASTER_OPTOUT` | `false` | An object owned by a master needs no signature |
| `FERRO_ACCEPT_TELEPORTS`, `FERRO_TELEPORT_LEVEL` | `true`, `Owner` | Teleport offers and requests: accepted at all, and from whom: `Owner` (the masters), `Friends` or `Anyone` |
| `FERRO_ACCEPT_GROUP_INVITES`, `FERRO_GROUP_INVITE_LEVEL` | `true`, `Owner` | Group invitations (never one with a fee) |
| `FERRO_ACCEPT_INVENTORY`, `FERRO_INVENTORY_LEVEL` | `true`, `Owner` | Inventory offers (else declined) |
| `FERRO_ACCEPT_FRIENDS`, `FERRO_FRIEND_LEVEL` | `true`, `Owner` | Friendship offers (`Friends` does not apply) |
| `FERRO_DIALOG_CHANNEL`, `FERRO_DIALOG_AVATAR`, `FERRO_DIALOG_HTTP` | (none) | Where script dialogs are relayed: a channel, an avatar's key, an address |
| `FERRO_DIALOG_OWNER` | (none) | Only the dialogs of this avatar's objects |
| `FERRO_HOME_SIM` | (none) | Homebound: the bot's home sim, `Region Name/x/y/z`; it goes back there |
| `FERRO_BACKUP_SIM` | (none) | Where it goes when home fails, or home restarts |
| `FERRO_HOME_AFTER` | `1` | Minutes away before it goes home |
| `FERRO_HOME_SEEK` | `true` | At home, go to the spot given |
| `FERRO_HOME_SIT` | (none) | At home, sit on this object (its key); a Stand command stops it until the next login |
| `FERRO_RATE_LIMIT` | `60` | Commands that change things in world (invites, ejects, chat…) per minute, bursts of 20; they wait their turn, up to a minute. `0`: no limit |
| `FERRO_RELAY_1`, `FERRO_RELAY_2`… | (none) | Relays, in SecondBot's short form: `sType::localchat{}sFilter::avatar{}tType::webhook{}tConfig::https://…` |
| `FERRO_DISCORD` | `false` | Discord: the bot on a Discord server (see "Discord" below) |
| `FERRO_DISCORD_SERVER`, `FERRO_DISCORD_TOKEN` | (none) | The Discord server's ID, the Discord bot's token (a secret, never logged) |
| `FERRO_DISCORD_MASTERS` | (none) | The Discord user IDs (comma-separated) whose words and commands the bot takes |
| `FERRO_DISCORD_COMMANDS` | `false` | The `commands` channel: a master's message there runs a command |
| `FERRO_DISCORD_HIDE_NAMES` | `false` | Lines sent to Second Life without the Discord user's name |
| `FERRO_DISCORD_CLEAR_ON_CONNECT` | `true` | The bot's channels emptied when it connects (`commands` kept) |
| `FERRO_DISCORD_INTERACTION`, `…_NAME`, `…_URL`, `…_CHANNEL` | `false`, `Go` | A message starting with the word, in that channel, posted to the address (signed) |
| `FERRO_ALLOW_FUNDS` | `false` | The money commands (Balance, PayAvatar, PayObject, ParcelBuy) |
| `FERRO_HEALTH_FILE` | `/tmp/ferrobot-health` | Where the bot writes its health, for `ferrobot healthcheck` |
| `RUST_LOG` | `info` | Log detail: `debug` shows every event, `trace` more |

SecondBot's names work too, so that a SecondBot container's settings can stay as they are:
`basic_Username`, `basic_Password`, `basic_LoginURI`, `commands_MastersCSV`,
`commands_SharedSecret`, `commands_TimeWindowSecs`, `commands_AllowIMcontrol`,
`commands_EnforceTimeWindow`, `commands_ObjectMasterOptout`, `commands_AllowFundsCommands`,
`http_Enabled`, `http_Port`,
`interaction_AcceptTeleports`, `interaction_TeleportRequestLevel`, `interaction_AcceptGroupInvites`,
`interaction_GroupInviteLevel`, `interaction_AcceptInventory`, `interaction_InventoryTransferLevel`,
`interaction_AcceptFriendRequests`, `interaction_FriendRequestLevel`, `dialogrelay_RelayChannel`,
`dialogrelay_RelayAvatar`, `dialogrelay_RelayHttpurl`, `dialogrelay_RelayObjectOwnerOnly`,
`homebound_Enabled`, `homebound_HomeSimSlUrl`, `homebound_BackupSimSLUrl`,
`homebound_ReturnToHomeSimAfterMins`, `homebound_AtHomeSeekLocation`, `homebound_AtHomeAutoSitUuid`,
`discord_Enabled`, `discord_ServerID`, `discord_ClientToken`, `discord_AllowDiscordCommands`,
`discord_hideChatterName`, `discord_ClearChatOnConnect`, `discord_InteractionEnabled`,
`discord_InteractionCommandName`, `discord_InteractionHttpTarget`,
`discord_InteractionChannelNumber`, and the relays:
`relay_count`, `relay_UseShortEncoder`, `relay_<n>_enabled`, `relay_<n>_extended`,
`relay_<n>_source_type`, `relay_<n>_source_option`, `relay_<n>_target_type`,
`relay_<n>_target_option`, `relay_<n>_RelayConfig`.

### Discord

The bot on a Discord server, as SecondBot's Discord service: a `bot` category with `status`
(its state), `localchat` and `commands`; an `im` category with a channel per avatar who IMs it
(`!close`, `!offertp`, `!clear`); a `group` category with a channel per group
(`!notice Title@@@Message[@@@item]`, `!eject`, `!invite`, `!ban`, `!clear`). What the bot hears
in Second Life is posted there; what a **Discord master** (`FERRO_DISCORD_MASTERS`) writes there
goes back to Second Life. Anyone else is answered ❌ and nothing reaches the bot.

1. In the Discord Developer Portal, make an application with a bot; turn on its **Message
   Content** intent (and **Server Members** for `Discord_MembersList`). Its token is
   `FERRO_DISCORD_TOKEN`.
2. Invite it to your server with these permissions: View Channels, Send Messages, Manage
   Channels, Manage Messages, Read Message History, Add Reactions (and, for the commands: Manage
   Roles, Ban Members, Mute Members).
3. Set `FERRO_DISCORD=true`, `FERRO_DISCORD_SERVER` (the server's ID: Developer Mode, right-click
   the server, Copy ID) and your own user ID in `FERRO_DISCORD_MASTERS`.

There is no Discord voice (`Discord_VoiceJoin` fails): it needs audio libraries the container
does not carry.

### Relays

What the bot hears, sent on (SecondBot's RelayService). Sources: `localchat` (filter: `avatar`,
`object`, `system`, `ownersay`, or a key), `avatar` (IMs; filter: the sender's key), `object`
(objects' IMs; filter: the owner's key), `group` (group chat; filter: the group's name or key).
Targets: `localchat` (a channel), `avatar` (a key), `group` (a key), `http` (SecondBot's signed
POST to `<url>/Relay/Service`), `webhook` (a JSON `{"content": …}` POST, as a Discord webhook
takes it). Relayed lines are marked `{Relay}` and never relayed again. The `FERRO_*` name
wins when both are set. `http_DisableCommandValidation` is ignored: commands are always signed.

### SecondBot's HTTP API

`POST /api/Run` with the form fields `commandName`, `args` (joined with `~#~`), `unixtime` and
`signing` = sha1(command + args + unixtime + shared secret), as SecondBot's. The answers are
SecondBot's: `{"Key": true, "Value": "{\"command\":…,\"reply\":…,\"status\":true}"}`, or a plain-text
refusal. The commands so far are the ones BB's SL group sync sends: `SimName`, `Name`, `Name2Key`,
`GetGroupList`, `GetGroupRoles`, `GetGroupMembers`, `GetGroupRoleMembers`, `IsGroupMember`,
`GroupInvite`, `GroupEject`, `GroupAddRole`, `GroupRemoveRole`; the other group commands:
`Groupnotice`, `GroupnoticeWithAttachment`, `GetGroupBanList`, `GroupBan`, `GetGroupTitles`,
`GroupActiveTitle`, `GroupActiveGroup`, `GroupJoin`, `GroupLeave`, `GetGroupMemberRoles`,
`ForceLoadGroups`; `RebakeAppearance`; the inventory: `InventoryFolders`,
`InventoryFoldersLimited`, `InventoryContents`, `SendItem`, `SendFolder`, `SendItemByPath`,
`SendFolderByPath`, `SendItemDirect`, `GetRealUUID`,
`CreateInventoryFolder`, `RenameInventory`, `DeleteInventoryItem` (to the Trash),
`DeleteInventoryFolder`, `MoveInventoryItem`, `MoveInventoryFolder`, `InventoryPurgeNotecards`
(the root's "Notecards" folder: notecards 31 days old or more, removed for good),
`InventoryContentsCatched` (the whole tree, fetched when asked: Ferrobot keeps no cache),
`InventoryPreloadCount`, `InventoryPreloadDone` (always "Ready": Ferrobot fetches on demand),
`SetInventoryUpdate` (a target hears of each item of a type that comes into the inventory);
`SetPermFlag` (SecondBot's answer, no effect, as SecondBot's); `ListSculptys` (owners and names
asked of the region);
`FetchNextNotecard` (StreamAdmin's next notecard, sent); notecards: `NotecardAdd`,
`NotecardClear`, `NotecardSend`, `NotecardDirectSend`, `NotecardRead` (the text to a reply target); `Teleport`, `TeleportSLURL`,
`SendTeleportLure`, `RequestTeleport`, `Key2Name`, `GetDisplayName`, `Nearme`, `Friendslist`,
`FriendRequest`, `FriendFullPerms`, `FriendsGetLocation`, `NearmeWithDetails`, `GetSimGlobalPos`,
`GetSimTexture`, `GetMuteList`, `SetAway`, `AsyncSendItem`, `AsyncSendItemByPath`,
`AsyncInventoryContents`, `Balance`, `PayAvatar`, `PayObject`; the parcel the bot stands on:
`GetCurrentParcelId`, `GetCurrentParcelUUID`, `GetCurrentParcelSize`, `GetParcelDesc`,
`GetParcelFlags`, `GetParcelPrimCounts`, `GetParcelSaleDetails`, `GetParcelTraffic`, `ParcelName`,
`GetCurrentRegionType`; the region's parcels: `UpdateListOfParcels`, `GetListOfParcels`,
`GetParcelListSnapshots`, `GetSimParcelLayers` (an SVG map); `QueryLandSaleData` (land for sale);
`SetParcelName`, `SetParcelDesc`, `SetParcelLandingZone`, `SetParcelMusic`, `SetParcelSale`,
`CancelParcelSale`, `SetParcelSnapshot`, `SetParcelAudioControls`, `SetParcelFlag`,
`ParcelSetMedia`, `ParcelEject`, `ParcelFreeze`, `ParcelBan`, `ParcelUnBan`, `GetParcelBanlist`,
`ResetParcelAcccessLists`, `ReturnAllObjectsOnParcel`, `ReturnAllObjectsOnParcelByOwner`,
`ParcelDeedToGroup`, `AbandonLand`, `ParcelBuy`, `EstateParcelReclaim`; the estate (for an estate
manager): `EstateKickUser`, `EstateTeleportHomeUser`, `AddEstateManager`, `RemoveEstateManager`,
`UpdateEstateBanlist`, `GetEstateBanList`, `SimRestart`, `SimMessage`, `SetEstateCovenant`,
`GetEstateCovenant`, `SetExtendedRegionInfo`, `SetRegionFlags`, `SetEstateConfig`,
`SetEstateTerrainRaw` (a RAW file from an http(s) URL, uploaded to the region); animations:
`PlayGesture`, `PlayGestureDirect`, `ResetAnimations`, `PointAt`, `AddToAllowAnimations` (that
avatar's objects may animate the bot); profiles: `GetProfileAbout`, `GetProfileImage`,
`SetProfileAbout`, `SetProfileImage`; objects: `RezObject`, `RezObjectOnPos`, `RezObjectParcelCenter`,
`AsyncRezObject`, `AsyncRezObjectOnPos`, `AsyncRezObjectParcelCenter`, `ClickObject`, `UnRezObject`,
`GetObjectsInParcel` (with each object's owner and name, asked of the region),
`TransferInventoryToObject`, `AsyncTransferInventoryToObject`; what it wears:
`Attach`, `Detach`, `SwapOutfit`, `SwapOutfitByPath`, `Outfit`; movement: `Fly`,
`SetAlwaysRun`, `Sit`, `Stand`, `AutoPilot`, `AutoPilotStop`, `RotateTo`, `RotateToFace`,
`RotateToFaceVector`, `HoverHeight`; `Hello`, `Version`, `UnixTimeNow`, `GetPosition`, `GoHome`, `Logoff`,
`GetLastCommands`, `ReadKeyValue`, `SetKeyValue`, `ClearKeyValue`; Ferrobot's own `BotStatus`
(in world, region, logins, uptime, commands run and failed); SecondBot's chat commands:
`LocalChatHistory`, `Say`, `Shout`, `Whisper`, `IM`, `chatwindows`, `listwithunread`,
`haveunreadims`, `getimchat`, `Groupchat`, `GroupchatHistory`, `GroupchatListAllUnreadGroups`,
`GroupchatGroupHasUnread`, `GroupchatAnyUnread`, `GroupchatClearAll`; Discord: `Discord_AddRole`,
`Discord_BanMember`, `Discord_GetMemberRoles`, `Discord_BulkClear_Messages`, `Discord_Dm_Member`,
`Discord_MembersList`, `Discord_MessageChannel`, `Discord_MuteMember`, `Discord_Role_GetSettings`,
`Discord_Role_UpdatePerms`, `Discord_RoleCreate`, `Discord_RoleList`, `Discord_RoleRemove`,
`Discord_TextChannels_List`, `Discord_VoiceJoin`, `Discord_VoiceLeave`; its dialog commands:
`DialogResponce`, `DialogRelayChatTarget`, `DialogRelayAvatarTarget`, `DialogRelayHttpTarget`.

An empty argument cannot be sent in the form: `args` is split on `~#~` with the empty parts left
out, before the signature is checked, as SecondBot does (so `a~#~` is the one argument `a`, and a
signature made over `a~#~` is refused).

`POST /json` is SecondBot's other entry: the body is one command, as a JSON object
`{"cmd": "Name2Key", "args": ["Some Resident"], "unixtime": 1800000000, "signing": "…", "reply": "7"}`
(`reply` optional: a channel, an avatar or an address that gets the answer too), or in the text form
of commands by IM, `Name2Key|||Some Resident#|#7@@@signature;|;unixtime`. The signature is the same
as `/api/Run`'s, and here empty arguments are kept. Every answer, the refusals ("No bot connected
yet", "No body sent", "Command request rejected") as much as `{"Key": …, "Value": …}`, comes as a
JSON string, as SecondBot's does: decode the body once to get SecondBot's text. Unlike SecondBot's,
the signature always carries the time and an older command is refused (SecondBot asks for it only
with `commands_EnforceTimeWindow`).

### Behind a Cloudflare Tunnel

The API is plain HTTP, made to sit behind a tunnel (or any reverse proxy) that does the HTTPS:

1. In your tunnel (Zero Trust > Networks > Tunnels), add a public hostname, say
   `bot.example.com`, whose service is the bot's port: `http://localhost:80` when cloudflared
   runs on the same host as a published port (then `FERRO_HTTP_BIND=127.0.0.1`, so that nothing
   else reaches it), `http://ferrobot:80` from the same Docker network.
   `deploy/compose.yaml` runs both in one network, with no port opened on the host.
2. In BB, the bot's address is `https://bot.example.com` (BB adds `/api/Run`), with the same
   shared secret as `FERRO_SHARED_SECRET`.
3. In Cloudflare, let BB's requests through: **Bot Fight Mode** and managed WAF challenges answer a
   script with a browser challenge page. Add a WAF custom rule *Skip* for the hostname (or
   `http.request.uri.path eq "/api/Run"`), and do not put Cloudflare Access in front of it.
   The commands are signed, so the endpoint does not need Cloudflare to guard it.
4. Check from outside: `curl https://bot.example.com/health` answers `ok` while the bot is in
   world (`offline` and 503 otherwise); nothing else is reachable without the secret.

A command never takes longer than 90 s, under Cloudflare's 100 s wait for the origin, so a slow
grid answers with a failure ("No reply from the grid [Retry later]") rather than Cloudflare's 524
page. The signature carries the time: the host's clock must be right (NTP), as with SecondBot.

In Second Life, mark the account as a **scripted agent** (account settings on secondlife.com), as the grid's rules ask of bots.

`ferrobot healthcheck` (the image's Docker `HEALTHCHECK`) exits with 0 while the bot is online, or
offline for less than 10 minutes (a relog), and with 1 otherwise.

Exit codes: `0` logged out on request, `1` bad settings, `2` a login refusal retrying can't fix (wrong password, terms of service not accepted, two-factor authentication on).

## Build and run

The image is published for amd64 and arm64:

```
docker run -d --name ferrobot --restart unless-stopped \
  -e FERRO_USERNAME='Bot Name' -e FERRO_PASSWORD='…' ghcr.io/blackbox-roleplay-systems/ferrobot:latest
```

Or build it yourself:

```
cargo build --release
docker build -t ferrobot .
docker run --rm -e FERRO_USERNAME='Bot Name' -e FERRO_PASSWORD='…' ferrobot
```

## Layout

- `crates/client` - the Second Life client, as a library: login, the region circuit, capabilities and
  the event queue, the state it keeps, its events
- `crates/ferrobot` - the bot: settings, the relog loop, its services
- `crates/llsd` - LLSD, Second Life's structured data (from Aurora Viewer)
- `crates/msg` - the UDP message system, generated from `message_template.msg` (from Aurora Viewer)
- `crates/testgrid` - a fake grid on localhost (login server, region, event queue) that the tests
  run the bot against

See `NOTICE.md` for what comes from where.

## Trying it without Second Life

`cargo run -p ferro-testgrid --example serve` starts the test grid, a fake grid on localhost, and
prints the settings to log the bot in to it (its own test account, not a Second Life one).

## Plan and contributing

- [ROADMAP.md](ROADMAP.md) - the plan, phase by phase
- [TASKS.md](TASKS.md) - where each step stands
- [ARCHITECTURE.md](ARCHITECTURE.md) - how the code is laid out
- [AGENTS.md](AGENTS.md) - rules for AI agents working on Ferrobot
