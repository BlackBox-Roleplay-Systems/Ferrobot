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
- 🟡 Published image (`.github/workflows/publish.yml`): `ghcr.io/kanikabreed/ferrobot`, amd64 and
  arm64, `latest` from main and versions from `v*` tags; runs on the first merge
- ⬜ Make the repository public (human)
- ✅ `scripts/check.sh`, `new-task.sh`, `end-task.sh`; AGENTS.md, ARCHITECTURE.md

## Phase 2 - The client becomes a library

Run on NGC (the local OpenSim server, 2026-11-12): login, arrival, event queue, an alert, a kick
then the relog, SIGTERM then the logout (exit 0), the health file; a region restart from NGC's
console (alert, "The simulator is going down.", the relog once it is back). Teleports not yet (one
region).

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
- ⬜ Relays: SecondBot's Discord bot (two ways) and RabbitMQ targets
- 🟡 Homebound (services/homebound.rs, SecondBot's HomeboundService): home sim after
  FERRO_HOME_AFTER minutes away, backup sim when home fails, evacuation on a restart, the spot at
  home, paused by a master's teleport until the next login; regions by name (MapNameRequest, as
  Aurora's teleport by name)
- ⬜ Homebound: sit on an object at home (AtHomeAutoSitUuid), with movement (phase 7)

## Phase 6 - Appearance and inventory

- 🟡 Server-side baking: once per login the region is asked to bake the current outfit
  (UpdateAvatarAppearance, the version it expects asked again, as Aurora's
  request_server_appearance); RebakeAppearance; tested on the test grid, to see in world
- ⬜ Hover height; changing the outfit (wear, take off)
- ⬜ Inventory fetch, give, accept
- ⬜ Notecards: create and send

## Phase 7 - The rest of SecondBot

- ⬜ Movement, avatars, friends, animations, parcels, estate, funds (order set by the human)

## Phase 8 - Running it for real

- 🟡 Status and metrics: `GET /health`, the BotStatus command (in world, region, logins, uptime,
  commands run and failed), one log line per command
- 🟡 Rate limit on the commands that change things (FERRO_RATE_LIMIT, 60 a minute, bursts of 20;
  they wait their turn up to a minute, then "Too many commands [Retry later]")
- 🟡 Published image with versioned tags (done in phase 1, see there)
