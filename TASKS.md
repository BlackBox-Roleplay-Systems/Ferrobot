# Tasks

✅ done (verified on the grid when it depends on it) · 🟡 done, not yet verified on the grid ·
🔧 in progress · ⬜ to do. The plan behind them is [ROADMAP.md](ROADMAP.md).

## Phase 0 - Foundations

- ✅ Workspace, GPL-3.0-or-later, NOTICE.md
- ✅ `crates/llsd` and `crates/msg` from Aurora Viewer b8cf1f047
- ✅ Login (XML-RPC), from aurora-net/src/login.rs: same-server redirects only, device ids derived
  from the account, permanent refusals (password, ToS, 2FA) end with exit code 2
- ✅ Region session: UseCircuitCode, CompleteAgentMovement, AgentThrottle, RegionHandshakeReply
  (flags 0x7, as Aurora and LibreMetaverse), pings, AgentUpdate every second, seed capability,
  EventQueueGet
- 🟡 The UDP socket is waited on until writable before the first packet: tokio refuses a send
  ("would block") until its reactor has seen the new socket, so UseCircuitCode was dropped now
  and then and the login waited 5 s for its resend (Aurora's circuit has the same send)
- ✅ Chat and IMs received are logged
- ✅ SIGTERM / Ctrl-C log out; a lost connection or a kick logs in again after `FERRO_RELOG_DELAY`
- 🟡 Staying power, as Aurora's session: the region is lost after 100 s of silence (not 60), the
  arrival waits 240 s and logs out before giving up, a lost event queue ends the session (relog),
  a closed queue no longer spins the session loop, and any end during a logout counts as logged out

## Phase 1 - First login, and a test grid

- ✅ First login on the grid (human, 2026-11-12): login and arrival, the HTTP API answering a
  signed SimName, a master's teleport offer, chat and IMs in the log, the relog after a kick
  ("logged in from another location"), the logout on Ctrl-C
- ⬜ Docker image built and run; `docker stop` logs out; image size noted
- ✅ `crates/testgrid`: fake login server (`login_to_simulator`, wrong password refused)
- ✅ `crates/testgrid`: fake region (circuit, handshake, arrival, pings both ways, logout, kick,
  DisableSimulator, local chat and IMs)
- ✅ `crates/testgrid`: seed capability and the held event queue (events, 502 when idle, 404 closed)
- ✅ Tests (`crates/ferrobot/tests/testgrid.rs`, the real binary): login, arrival, chat, IM, events,
  pings, SIGTERM logout (exit 0), kick and closed circuit then relog, no relog (exit 1), wrong
  password (exit 2), start location, a closed event queue that neither ends the session nor spins
- ⬜ Test grid: a region that goes silent (the 100 s timeout), and an event queue that gives up;
  both need the bot's timeouts settable for tests
- 🟡 GitHub CI (`.github/workflows/ci.yml`): `check.sh`, the Docker image built and started, its
  size in the run summary; runs on the first PR
- 🟡 Published image (`.github/workflows/publish.yml`): `ghcr.io/blackbox-roleplay-systems/ferrobot`, amd64 and
  arm64, `latest` from main and versions from `v*` tags; runs on the first merge
- ⬜ Make the repository public (human)
- ✅ `scripts/check.sh`, `new-task.sh`, `end-task.sh`; AGENTS.md, ARCHITECTURE.md

## Phase 2 - The client becomes a library

Run on NGC (the local OpenSim server, 2026-11-12): login, arrival, event queue, an alert, a kick
then the relog, SIGTERM then the logout (exit 0), the health file; a region restart from NGC's
console (alert, "The simulator is going down.", the relog once it is back). Teleports not yet (one
region).
Run on NGC again (2026-11-12, feat/more-commands): the API answering SimName, BotStatus, Balance,
Nearme, Friendslist; the inventory tree fetched from the server; a folder created; a notecard
written (the server's "Notecard updated") and listed; both thrown away. OpenSim has no server-side
baking: the bot says so and goes on.

- ✅ `crates/client` (ferro-client): the protocol code moved out of the bot; `Client` (requests with
  replies and a timeout, a broadcast of events, the state) and `Network` (the network task, one
  session at a time); the bot's chat and IM log is now a service on the events
- ✅ Teleports (`Client::accept_lure`, `teleport_home`, `teleport_to`): TeleportLureRequest,
  TeleportLandmarkRequest, TeleportLocationRequest; TeleportFinish and CrossedRegion move the
  circuit and the event queue to the new region (Aurora's switch_region), TeleportFailed and
  TeleportLocal handled; tested on the test grid's two regions; a master's lure verified on the
  grid 2026-11-12
- ✅ Masters' teleport offers accepted (`FERRO_MASTERS`, now services/interactions.rs, SecondBot's
  BotTeleportOffer); others ignored; verified on the grid 2026-11-12
- ✅ Neighbour regions: not connected, like LibreMetaverse's bots (`MultipleSims = false`); a
  bot that stays put needs no child agents. Revisit with movement (phase 7)
- 🟡 Region restarts: notices read as Aurora's region_restart.rs (Second Life's named alerts,
  OpenSim's `/RESTART_X_…` texts, the cancellation); the bot goes home at once when home is in
  another region (services/restart.rs), else logs in again after the restart
- 🟡 The bot's state (`ferro_client::State`): online, region, position, home (login reply), active
  group (AgentDataUpdate, as LLAgent asks for it), groups (AgentGroupDataUpdate on the event queue
  or UDP, AgentDropGroup), as Aurora's session_social.rs; tested on the test grid
- 🟡 Names: key -> name (UUIDNameRequest) and name -> key (AvatarPickerRequest, a username meaning
  "<name> Resident" like SecondBot's Name2Key), cached; tested on the test grid
- 🟡 Display names (`Client::display_names`): the GetDisplayNames capability as Aurora's
  request_display_names and LLAvatarName::fromLLSD, cached an hour; legacy names where the grid
  has no People API (OpenSim); tested on the test grid
- 🟡 Health state in the client (`Client::health`: online, region, time since the last packet)
- ✅ Docker HEALTHCHECK: the bot writes a health file (`FERRO_HEALTH_FILE`), `ferrobot healthcheck`
  reads it (healthy while online, or offline under 10 min); tested against the test grid

## Phase 3 - Groups

- 🟡 The bot's groups and the active group (phase 2's state)
- 🟡 Roles and titles (`Client::group_roles`, `group_titles`), replies gathered over packets
- 🟡 Members (`group_members`): GroupMemberData capability, GroupMembersRequest without it; on
  demand, cached a minute (SecondBot keeps the login's list)
- 🟡 Role members (`group_role_members`: every role / member pair)
- 🟡 Invite with a role, eject (answered by EjectGroupMemberReply), give and take a role
- 🟡 Is someone a member (`is_group_member`)
- 🟡 Answer a group invitation (`answer_group_invite`); from crates/client/src/groups.rs, a port
  of Aurora's groups.rs, and network/groups.rs as its session_groups.rs. Tested on the test grid
- 🟡 Group invitations accepted from masters (now services/interactions.rs, SecondBot's
  BotGroupInviteOffer at level "Owner"), others left unanswered; deliberate difference: never one
  with a membership fee
- 🟡 Group chat sessions (`group_chat_join`, `group_chat_send`, `group_chat_leave`;
  `Event::GroupChat`): IM_SESSION_GROUP_START and ChatterBoxSessionStartReply, IM_SESSION_SEND,
  ChatterBoxInvitation accepted through ChatSessionRequest, as Aurora and the viewer's LLIMMgr;
  logged by the chat log

## Phase 4 - SecondBot's HTTP API

- ✅ `/api/Run` (src/api, axum): form fields, sha1 signature, 35 s window, `{Key, Value}` answers,
  the plain-text refusals, "Unknown command", the argument count, `{"Message":"Error"}` elsewhere;
  as SecondBot's HttpService and CommandsService. Off without a shared secret (deliberate)
- 🟡 BB's commands (src/commands): SimName, Name, Name2Key, GetGroupList, GetGroupRoles,
  GetGroupMembers, GetGroupRoleMembers, IsGroupMember, GroupInvite, GroupEject, GroupAddRole,
  GroupRemoveRole; replies, failures and args as SecondBot's Commands/Group.cs, Info.cs,
  Avatars.cs; tested through the API against the test grid. Deliberate differences: lists are
  fetched when asked, "Updating" instead of an empty list when the grid does not answer;
  GroupEject waits for the grid ("Not in group", "Missing group Eject power"); GroupAddRole
  refuses an unknown avatar; Name2Key asks the grid at once
- ✅ SecondBot setting names accepted next to FERRO_* (basic_*, commands_*, http_*)
- 🟡 SecondBot's other group commands: Groupnotice, GroupnoticeWithAttachment, GetGroupBanList,
  GroupBan (GroupAPIv1), GetGroupTitles, GroupActiveTitle, GroupActiveGroup, GroupJoin,
  GroupLeave, GetGroupMemberRoles, ForceLoadGroups; tested through the API on the test grid.
  Not GroupCreate (it costs L$100)
- 🟡 SecondBot's Hello, Version, UnixTimeNow, GetPosition, GoHome, Logoff, GetLastCommands,
  ReadKeyValue, SetKeyValue, ClearKeyValue. Deliberate differences: SetKeyValue and ClearKeyValue
  work (SecondBot's check is inverted), GetLastCommands gives the newest five
- ⬜ SecondBot's `/json` endpoint (a signed JSON body); not used by BB
- 🟡 Behind a Cloudflare Tunnel: 90 s command deadline (Cloudflare waits 100 s), `GET /health`,
  `deploy/compose.yaml` with cloudflared, README section; to verify through a real tunnel (human)
- ✅ BB's bot tests pass against Ferrobot on the test grid (2026-11-12): BB's own SlBot.php,
  unchanged, through the checks of the harness that built the SL group sync, 23 of 23
  (`cargo run -p ferro-testgrid --example serve` runs the test grid by itself)
- ⬜ BB dev site runs its SL group sync on Ferrobot (human)

## Phase 5 - Talking and listening

- 🟡 Send chat, IMs, group chat; history commands (`Client::chat`, `im`; src/history.rs as
  SecondBot's DataStoreService; its Chat and Groupchat commands: LocalChatHistory, Say, Shout,
  Whisper, IM, chatwindows, listwithunread, haveunreadims, getimchat, Groupchat,
  GroupchatHistory, GroupchatListAllUnreadGroups, GroupchatGroupHasUnread, GroupchatAnyUnread,
  GroupchatClearAll); tested through the API against the test grid
- 🟡 Masters and interactions (services/interactions.rs, SecondBot's InteractionService): teleport
  offers and requests, group invitations, inventory offers, friendship offers; a switch and a
  level (Owner, Friends, Anyone) for each; the friends from the login's buddy list
- ⬜ SecondBot's one-time "next accept" list and its JSON interaction events
- 🟡 Commands by IM (services/im_commands.rs, SecondBot's BotImMessage and SignedCommand): from
  masters unsigned, from objects signed (FERRO_OBJECT_MASTER_OPTOUT), the text and JSON forms,
  the reply target (channel, avatar, HTTP POST; not e-mail). Deliberate difference: a master is
  recognised by key, not by the name an IM carries (also for teleport offers)
- 🟡 Script dialogs (src/dialogs.rs, SecondBot's DialogService and Dialogs commands): numbered,
  440 s, relayed as its JSON to channels, avatars and HTTP (signed); DialogResponce,
  DialogRelayChatTarget, DialogRelayAvatarTarget, DialogRelayHttpTarget. Deliberate difference:
  expired dialogs are cleaned, not the live ones
- 🟡 Relays (services/relays.rs, SecondBot's RelayService): local chat, avatar and object IMs,
  group chat, to local chat, an avatar, a group, HTTP (signed) or a webhook (Discord's format);
  SecondBot's relay settings and the short form; tested on the test grid
- 🟡 Discord (crates/ferrobot/src/discord/, SecondBot's DiscordService, on twilight's gateway and
  REST client): the `bot`, `im` and `group` categories and their channels, made when missing;
  local chat, object IMs, an avatar's IMs and a group's chat posted there, the bot's state in
  `status` and its region as its activity; back from Discord masters: local chat, IMs (`!close`,
  `!offertp`, `!clear`), group chat (`!notice`, `!eject`, `!invite`, `!ban`, `!clear`), the
  `commands` channel (the signed text form, as by IM), the interaction (a signed post to an
  address); SecondBot's Discord_* commands. Tested against a stand-in Discord on localhost
  (crates/testgrid/src/discord.rs). Deliberate differences: only Discord masters
  (FERRO_DISCORD_MASTERS) speak through the bot or command it (SecondBot takes anyone on the
  server), the `commands` channel follows FERRO_DISCORD_COMMANDS (SecondBot never reads its
  AllowDiscordCommands), the lists answer as JSON (SecondBot prints C# type names), the inverted id
  checks of four commands fixed, no voice (no audio library in the container)
- ⬜ Discord: to try on a real server (human)
- ⬜ Test grid: a login sometimes takes 5 s (seen in any test, Discord or not)
- ⬜ Relays: RabbitMQ targets
- 🟡 Homebound (services/homebound.rs, SecondBot's HomeboundService): home sim after
  FERRO_HOME_AFTER minutes away, backup sim when home fails, evacuation on a restart, the spot at
  home, paused by a master's teleport until the next login; regions by name (MapNameRequest, as
  Aurora's teleport by name)
- 🟡 Homebound sits on its seat at home (FERRO_HOME_SIT / homebound_AtHomeAutoSitUuid), asking
  again every 30 s while not seated, not seeking the spot while seated; a Stand command stops it
  until the next login (SecondBot's MarkStandup lasts until it restarts); tested on the test grid

## Phase 6 - Appearance and inventory

- ✅ Server-side baking: once per login the region is asked to bake the current outfit
  (UpdateAvatarAppearance, the version it expects asked again, as Aurora's
  request_server_appearance); RebakeAppearance; verified on the grid 2026-11-12 (never a cloud)
- 🟡 Hover height (HoverHeight, the AgentPreferences capability, as the viewer's
  LLVOAvatarSelf::sendHoverHeight); tested on the test grid
- ⬜ Changing the outfit (wear, take off)
- 🟡 Inventory (crates/client/src/inventory.rs from Aurora's inventory.rs; FetchInventoryDescendents2,
  FetchInventory2; MoveInventoryItem, MoveInventoryFolder, CreateInventoryFolder; gifts as
  give_inventory): SecondBot's InventoryFolders, InventoryFoldersLimited, InventoryContents,
  SendItem, SendFolder, GetRealUUID, CreateInventoryFolder, RenameInventory, DeleteInventoryItem,
  DeleteInventoryFolder, MoveInventoryItem, MoveInventoryFolder; accepting offers is the
  interaction rules'. Deliberate difference: deleting moves to the Trash
- 🟡 Inventory by path and direct: SecondBot's SendItemByPath, SendFolderByPath (a folder fetched
  per part, exact names, as its FindObjectByPath), SendItemDirect (its AssetType names, and the
  viewer's short ones its hints give); tested on the test grid. Deliberate difference: empty parts
  of a path are skipped ("/Stuff/Sub/" is "Stuff/Sub")
- 🟡 AsyncSendItem, AsyncSendItemByPath, AsyncInventoryContents: a request id at once, SecondBot's
  InventoryAsyncReply to the reply target; tested on the test grid
- ⬜ Inventory: rezzing, attaching, outfits, AsyncRez…, transfers to objects
- 🟡 Notecards: created (CreateInventoryItem, UpdateNotecardAgentInventory's uploader, the
  "Linden text version 2" asset, as Aurora) and sent; SecondBot's NotecardAdd, NotecardClear,
  NotecardSend, NotecardDirectSend; tested on the test grid
- 🟡 NotecardRead: the asset over UDP as the viewer reads a protected one (TransferRequest from
  the inventory, crates/client/src/transfer.rs from Aurora's inventory_preview.rs), its text to the
  reply target (SmartCommandReply, now shared with the commands by IM); tested on the test grid,
  and on NGC (a notecard the bot wrote, read back by HTTP reply).
  Deliberate difference: "[ATTACHMENT]" stands where each embedded item is (SecondBot's loop
  drops the text before the last one)

## Phase 7 - The rest of SecondBot

- 🟡 Teleports by command (Teleport, TeleportSLURL, SendTeleportLure, RequestTeleport), names
  (Key2Name, GetDisplayName), avatars nearby (Nearme, from CoarseLocationUpdate), friends
  (Friendslist with who is online, FriendRequest, FriendFullPerms), funds (Balance, PayAvatar,
  PayObject; off unless FERRO_ALLOW_FUNDS, as SecondBot's AllowFundsCommands); tested on the test
  grid. Deliberate difference: Key2Name works (SecondBot's looks up the nil key)
- 🟡 Movement (commands/movement.rs; control flags in the agent update, AgentRequestSit then
  AgentSit, SetAlwaysRun, the "autopilot" GenericMessage in global coordinates, as the viewer's
  LLAgent): SecondBot's Fly, SetAlwaysRun, Sit (an object or the ground), Stand, AutoPilot (turns
  toward the target first, as its TurnToward), AutoPilotStop, RotateTo, RotateToFace (the avatar's
  coarse location), RotateToFaceVector, HoverHeight; tested on the test grid. Deliberate difference: RotateTo works (SecondBot's check is inverted, it refuses
  every number); the heading is in degrees clockwise from north
- 🟡 The bot's own place follows its avatar's object updates (crates/client/src/objects.rs, from
  Aurora's objects.rs: full, compressed and terse updates; seated, the seat's place plus the
  offset, as SecondBot's AvatarPositions). Tried on NGC: flying, landing and the autopilot moved
  it, and GetPosition matched the region's own position
- ⬜ Movement: walking by keys (forward, back, turn) and following an avatar
- 🟡 SecondBot's NearmeWithDetails (the avatars' object updates, seated ones on their seats; every
  avatar's place is kept now), FriendsGetLocation (FindAgent, as LLAvatarTracker::findAgent),
  GetSimGlobalPos and GetSimTexture (the world map), GetMuteList (asked at each login, downloaded
  by xfer as Aurora's session_social.rs), SetAway (the viewer's setAFK: the away animation and
  AGENT_CONTROL_AWAY); tested on the test grid. Deliberate difference: FriendsGetLocation waits up
  to 3 s for the map's first answer (SecondBot fails the first call and answers the next)
- 🟡 The parcel the bot stands on (crates/client/src/parcels.rs from Aurora's land.rs:
  ParcelProperties with sequence 0, asked for with ParcelPropertiesRequest when missing; its
  dwell by ParcelDwellRequest, its global key by RemoteParcelRequest): SecondBot's
  GetCurrentParcelId, GetCurrentParcelUUID, GetCurrentParcelSize, GetParcelDesc, GetParcelFlags,
  GetParcelPrimCounts, GetParcelSaleDetails, GetParcelTraffic; tested on the test grid.
  Deliberate difference: GetParcelTraffic asks the region (SecondBot prints 0 until something did)
  Tried on NGC: every getter answered from the region's own data, and the ban list round trip
- 🟡 Parcel changes (the ParcelPropertiesUpdate capability, the whole parcel with what changed,
  as LLParcel::packMessage): SecondBot's SetParcelName, SetParcelDesc, SetParcelLandingZone,
  SetParcelMusic, SetParcelSale, CancelParcelSale, SetParcelSnapshot, SetParcelAudioControls,
  SetParcelFlag, ParcelSetMedia; tested on the test grid. Deliberate differences: SetParcelFlag
  skips a value that is not true or false (SecondBot's throws), ParcelSetMedia keeps sizes in
  256..1024 (SecondBot's check lets any through)
- 🟡 Parcel access (EjectUser, FreezeUser, ParcelAccessListRequest/Update as the viewer's
  sendParcelAccessListUpdate, ParcelObjectOwnersRequest then ParcelReturnObjects, ParcelDeedToGroup,
  ParcelRelease, ParcelBuy, ParcelReclaim; the estate manager flag from RegionHandshake):
  SecondBot's ParcelEject, ParcelFreeze, ParcelBan, ParcelUnBan, GetParcelBanlist,
  ResetParcelAcccessLists, ReturnAllObjectsOnParcel, ReturnAllObjectsOnParcelByOwner,
  ParcelDeedToGroup, AbandonLand, ParcelBuy, EstateParcelReclaim; tested on the test grid.
  Deliberate differences: ReturnAllObjectsOnParcelByOwner returns that owner's objects (SecondBot's
  returns everyone else's), bans never end (SecondBot's end time overflows), ParcelBuy is off
  unless FERRO_ALLOW_FUNDS (it spends money)
- 🟡 The estate (crates/client/src/estate.rs from Aurora's estate.rs: EstateOwnerMessage with its
  invoice, `getinfo` answered by `estateupdateinfo`, the EstateAccess, DispatchRegionInfo and
  EstateChangeInfo capabilities with the viewer's EstateOwnerMessage fallbacks, the covenant by
  EstateCovenantRequest and a transfer from the estate): SecondBot's EstateKickUser,
  EstateTeleportHomeUser, AddEstateManager, RemoveEstateManager, UpdateEstateBanlist,
  GetEstateBanList, SimRestart, SimMessage, SetEstateCovenant, GetEstateCovenant,
  SetExtendedRegionInfo, SetRegionFlags, SetEstateConfig; tested on the test grid. Deliberate
  differences: SetEstateConfig keeps the estate's other flags and sends the sun hour to the
  capability as the viewer does. Tried on NGC as a resident: GetEstateCovenant read the estate,
  EstateAccess refused (401) as it should for one who does not manage it; the changes need an
  estate manager (human)
- ⬜ SetEstateTerrainRaw (a terrain upload by xfer), QueryLandSaleData, the region's parcel list
  (GetListOfParcels, UpdateListOfParcels, GetParcelListSnapshots, GetSimParcelLayers)
- 🟡 Objects (every object the region tells of kept by key, crates/client/src/objects.rs; RezObject
  as LLToolDragAndDrop::dropObject and Aurora's inventory/rez.rs, with the item's CRC; a click as
  LLToolGrab's; DeRezObject; UpdateTaskInventory and RezScript as dropInventory): SecondBot's
  RezObject, RezObjectOnPos, RezObjectParcelCenter and their Async… forms, ClickObject, UnRezObject,
  GetObjectsInParcel, TransferInventoryToObject and its Async… form; tested on the test grid.
  Deliberate differences: ClickObject answers "ok" or fails when it cannot see the object
  (SecondBot prints a C# pair), GetObjectsInParcel lists ID, LocalID, OwnerID and Position of each
  root object by the parcel's x and y (SecondBot serialises LibreMetaverse's whole Primitive and
  also compares the height with the parcel's 0, so finds nothing). Tried on NGC: GetObjectsInParcel
  listed the region's objects
- ⬜ Objects' owners and names (ObjectPropertiesFamily): an object update names its owner only
  for an object with a sound, so GetObjectsInParcel's OwnerID is often nil
- 🟡 What the bot wears, through the Current Outfit Folder (crates/client/src/outfit.rs, as
  LibreMetaverse's AppearanceManager and CurrentOutfitFolder: LinkInventoryItem and
  RemoveInventoryItem for its links, RezSingleAttachmentFromInv to add an object,
  DetachAttachmentIntoInv; what is worn from the attachments' object updates, their point and
  AttachItemID): SecondBot's Attach, Detach (a key, a point's name, or "*"), SwapOutfit,
  SwapOutfitByPath, Outfit; an outfit's clothing replaces all, its body parts those of their types,
  its objects the objects worn (one request at a time), then a rebake; tested on the test grid.
  Deliberate difference: Detach "*" takes off everything (SecondBot's skips all)
- 🟡 Animations and gestures: PlayGesture and PlayGestureDirect (the asset by ViewerAsset, decoded
  as Aurora's gesture.rs, its steps played as LLGestureMgr: animations, sounds by SoundTrigger,
  chat with its channel, waits; its animations stopped at its end), ResetAnimations (what
  AvatarAnimation says the bot plays; Stand does it too, as SecondBot's), PointAt (the point
  animation, LLHUDEffectPointAt and a beam), AddToAllowAnimations (ScriptQuestion: that avatar's
  objects get PERMISSION_TRIGGER_ANIMATION, nothing more); tested on the test grid. Deliberate
  differences: AddToAllowAnimations works (SecondBot's answers "@todo"), PointAt finds the avatar
  (SecondBot's reads the key before looking it up)
- 🟡 Profiles (the AgentProfile capability, GET and PUT, else AvatarPropertiesRequest and the
  whole profile in AvatarPropertiesUpdate, as Aurora's session for OpenSim): SecondBot's
  GetProfileAbout, GetProfileImage, SetProfileAbout, SetProfileImage; tested on the test grid (NGC
  has its profile service off: no viewer sees profiles there either)

## Phase 8 - Running it for real

- 🟡 Status and metrics: `GET /health`, the BotStatus command (in world, region, logins, uptime,
  commands run and failed), one log line per command
- 🟡 Rate limit on the commands that change things (FERRO_RATE_LIMIT, 60 a minute, bursts of 20;
  they wait their turn up to a minute, then "Too many commands [Retry later]")
- 🟡 Published image with versioned tags (done in phase 1, see there)
