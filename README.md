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
| `FERRO_INTERACTION_EVENTS` | `false` | Each offer told as SecondBot's interaction JSON (accepted or not, why), and the items that come in |
| `FERRO_INTERACTION_EVENTS_TARGET` | (none) | Where those events go, as a command's reply target: a channel, an avatar (key or name), an http(s) address (a form POST of `reply`, `command=interactions`), an e-mail address (with `FERRO_SMTP_REPLY_TARGET`) |
| `FERRO_EVENTS` | `false` | SecondBot's events service (see "Events" below) |
| `FERRO_EVENTS_CHANNEL`, `FERRO_EVENTS_AVATAR`, `FERRO_EVENTS_HTTP` | (none) | Where the events go: a channel (0 or more), an avatar's key, an http(s) address (a signed POST to `<address>/Event/Service`) |
| `FERRO_EVENTS_SECRET` | SecondBot's | What signs each event's `Hash` (a secret, never logged). Set your own |
| `FERRO_EVENTS_GROUP` | (none) | The group whose members joining and leaving are told |
| `FERRO_EVENTS_GROUP_JOINS`, `FERRO_EVENTS_GROUP_LEAVES` | `true` | Each of them |
| `FERRO_EVENTS_GUEST_SIM`, `FERRO_EVENTS_GUEST_PARCEL` | (none) | Guests are told while the bot stands on this parcel of this region (their names) |
| `FERRO_EVENTS_GUEST_ENTERS`, `FERRO_EVENTS_GUEST_LEAVES` | `true` | Each of them |
| `FERRO_EVENTS_CHANGE_SIM`, `FERRO_EVENTS_CHANGE_PARCEL`, `FERRO_EVENTS_ALERTS`, `FERRO_EVENTS_STATUS`, `FERRO_EVENTS_MONEY` | `true` | The other events, each on or off |
| `FERRO_EVENTS_GROUP_CHECK`, `FERRO_EVENTS_GUEST_CHECK` | `120`, `30` | Seconds between two looks at the group's members, at the guests (SecondBot's pace) |
| `FERRO_ON_EVENT` | `false` | Commands run on events (see "Commands on events" below) |
| `FERRO_ON_EVENT_<n>_SOURCE`, `…_MONITOR`, `…_WHERE_<m>`, `…_ACTION_<m>`, `…_ENABLED` | (none), `true` | Event `n` (1, 2…): its source, what it watches, its conditions and actions (1, 2…) |
| `FERRO_ON_EVENT_WAIT` | `10` | Seconds after each login before events run |
| `FERRO_ON_EVENT_DEBUG` | `false` | Each step of the events in the log (the IMs' text too) |
| `FERRO_ON_EVENT_GUEST_CHECK`, `FERRO_ON_EVENT_GROUP_CHECK` | `10`, `900` | Seconds between two looks at the region's avatars, at each group's members (one group a minute at most) |
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
| `FERRO_RABBIT` | `false` | RabbitMQ: the bot on an AMQP broker (see "RabbitMQ" below) |
| `FERRO_RABBIT_HOST`, `FERRO_RABBIT_PORT` | `127.0.0.1`, `5672` | The broker (`5671` by default with TLS) |
| `FERRO_RABBIT_USERNAME`, `FERRO_RABBIT_PASSWORD` | `guest`, `guest` | Its account (the password is a secret, never logged) |
| `FERRO_RABBIT_VHOST` | `/` | Its virtual host |
| `FERRO_RABBIT_TLS` | `false` | amqps: TLS, the certificate checked as for HTTPS |
| `FERRO_RABBIT_NOTECARD_QUEUE`, `…_COMMAND_QUEUE`, `…_IM_QUEUE`, `…_GROUP_IM_QUEUE` | `notecards`, `commands`, `ims`, `groupims` | The queues the bot reads (signed messages only) |
| `FERRO_CHATGPT` | `false` | AI replies through an OpenAI-compatible chat completions API (see "AI replies" below) |
| `FERRO_CHATGPT_API_KEY` | (none) | The API's key (a secret, never logged); required for OpenAI |
| `FERRO_CHATGPT_PROVIDER` | `openai` | `openai`, or another provider's address: requests go to `<address>/v1/chat/completions` |
| `FERRO_CHATGPT_ORGANIZATION` | (none) | OpenAI's organization, if any |
| `FERRO_CHATGPT_MODEL` | `gpt-3.5-turbo` | The model, as the provider names it |
| `FERRO_CHATGPT_PROMPT` | SecondBot's horse | What the bot is told to be: "You are <name>, <prompt>, you are talking to …" |
| `FERRO_CHATGPT_NAME` | the bot's first name | The name the bot is told it has |
| `FERRO_CHATGPT_HISTORY`, `FERRO_CHATGPT_HISTORY_TIMEOUT` | `5`, `15` | Lines a conversation keeps (3 to 10), minutes before a quiet one is forgotten |
| `FERRO_CHATGPT_LOCALCHAT`, `…_LOCALCHAT_COOLDOWN` | `false`, `3` | Answer local chat; seconds between two questions (1 to 10, for everyone) |
| `FERRO_CHATGPT_IM`, `…_IM_FRIENDS_ONLY`, `…_IM_COOLDOWN` | `false`, `false`, `3` | Answer IMs, from friends only; seconds between two questions of an avatar |
| `FERRO_CHATGPT_GROUP`, `…_GROUP_KEY`, `…_GROUP_COOLDOWN` | `false`, (none), `3` | Answer one group's chat (its key); seconds between two questions there |
| `FERRO_CHATGPT_TYPING` | `true` | Wait 1 to 3 s before replying, with the typing animation in local chat |
| `FERRO_CHATGPT_DEBUG` | `false` | Log what is asked and answered at `info` (else `debug`) |
| `FERRO_CUSTOM_COMMAND_1_TRIGGER`, `…_1_ARGS`, `…_1_STEP_1`, `…_1_STEP_2`… | (none), `0` | A custom command: its name, the arguments it takes, its steps (see "Custom commands" below); `FERRO_CUSTOM_COMMAND_2_…` for the next |
| `FERRO_SMTP` | `false` | E-mail through an SMTP server (see "E-mail" below) |
| `FERRO_SMTP_HOST`, `FERRO_SMTP_PORT` | (none), `587` | The SMTP server and its port |
| `FERRO_SMTP_TLS` | `starttls` | `starttls` (must succeed), `tls` (from the first byte, usually port 465), or `none` (only for a relay on the same host or network) |
| `FERRO_SMTP_USER`, `FERRO_SMTP_PASSWORD` | (none) | The login on the server; without a password (or `none`) the bot does not log in. The password is a secret, never logged |
| `FERRO_SMTP_FROM` | (none) | The address mail comes from; nothing is sent without it |
| `FERRO_SMTP_COMMAND` | `false` | The `SendEmail` command |
| `FERRO_SMTP_REPLY_TARGET` | `false` | An e-mail address as a command's reply target |
| `FERRO_SMTP_ALLOWED` | (none: any) | The only addresses mailed, comma-separated |
| `FERRO_SMTP_ALERTS`, `FERRO_SMTP_LOGIN_NOTICE` | `false`, `false` | The grid's alerts, and each login, mailed to `FERRO_SMTP_NOTIFY` |
| `FERRO_SMTP_NOTIFY` | (none) | Where alerts and login notices go |
| `FERRO_ALLOW_FUNDS` | `false` | The money commands (Balance, PayAvatar, PayObject, ParcelBuy, GroupCreate, and UploadMediaWave unless the grid's sounds are free) |
| `FERRO_ALLOW_SERVICE_CONTROL` | `false` | The service control commands (EnableAndStartService, StopService, RestartService, ChangeConfigValue) |
| `FERRO_RLV` | `false` | RLV: the bot's own objects (and its masters') drive it with `@` commands (see "RLV" below). Homebound is off while it is on, as SecondBot's |
| `FERRO_HEALTH_FILE` | `/tmp/ferrobot-health` | Where the bot writes its health, for `ferrobot healthcheck` |
| `RUST_LOG` | `info` | Log detail: `debug` shows every event, `trace` more |

SecondBot's names work too, so that a SecondBot container's settings can stay as they are:
`basic_Username`, `basic_Password`, `basic_LoginURI`, `commands_MastersCSV`,
`commands_SharedSecret`, `commands_TimeWindowSecs`, `commands_AllowIMcontrol`,
`commands_EnforceTimeWindow`, `commands_ObjectMasterOptout`, `commands_AllowFundsCommands`,
`commands_AllowServiceControl`,
`http_Enabled`, `http_Port`,
`interaction_AcceptTeleports`, `interaction_TeleportRequestLevel`, `interaction_AcceptGroupInvites`,
`interaction_GroupInviteLevel`, `interaction_AcceptInventory`, `interaction_InventoryTransferLevel`,
`interaction_AcceptFriendRequests`, `interaction_FriendRequestLevel`,
`interaction_EnableJsonOutputEvents`, `interaction_JsonOutputEventsTarget`, `dialogrelay_RelayChannel`,
`dialogrelay_RelayAvatar`, `dialogrelay_RelayHttpurl`, `dialogrelay_RelayObjectOwnerOnly`,
`homebound_Enabled`, `homebound_HomeSimSlUrl`, `homebound_BackupSimSLUrl`,
`homebound_ReturnToHomeSimAfterMins`, `homebound_AtHomeSeekLocation`, `homebound_AtHomeAutoSitUuid`,
`discord_Enabled`, `discord_ServerID`, `discord_ClientToken`, `discord_AllowDiscordCommands`,
`discord_hideChatterName`, `discord_ClearChatOnConnect`, `discord_InteractionEnabled`,
`discord_InteractionCommandName`, `discord_InteractionHttpTarget`,
`discord_InteractionChannelNumber`, `rabbit_Enabled`, `rabbit_HostIP`, `rabbit_HostPort`,
`rabbit_HostUsername`, `rabbit_HostPassword`, `rabbit_NotecardQueue`, `rabbit_CommandQueue`,
`rabbit_ImQueue`, `rabbit_GroupImQueue`, `chatgpt_Enabled`, `chatgpt_ApiKey`,
`chatgpt_OrganizationId`, `chatgpt_Provider`, `chatgpt_UseModel`, `chatgpt_ChatPrompt`,
`chatgpt_CustomName`, `chatgpt_ChatHistoryMessages`, `chatgpt_ChatHistoryTimeout`,
`chatgpt_LocalchatReply`, `chatgpt_LocalchatRateLimiter`, `chatgpt_AllowImReplys`,
`chatgpt_ImReplyFriendsOnly`, `chatgpt_ImReplyRateLimiter`, `chatgpt_AllowGroupReplys`,
`chatgpt_GroupReplyForGroup`, `chatgpt_GroupReplyRateLimiter`, `chatgpt_FakeTypeDelay`,
`chatgpt_ShowDebug`, the events: `events_Enabled`, `events_OutputChannel`, `events_OutputIMuuid`,
`events_OutputHttpURL`, `events_OutputSecret`, `events_GroupMemberEventsGroupUUID`,
`events_GroupMemberJoins`, `events_GroupMemberLeaves`, `events_GuestTrackingSimname`,
`events_GuestTrackingParcelname`, `events_GuestEntersArea`, `events_GuestLeavesArea`,
`events_ChangeSim`, `events_ChangeParcel`, `events_SimAlertMessage`, `events_StatusMessage`,
`events_MoneyEvent`; the commands on events: `onevent_Enabled`, `onevent_WaitSecsToStart`,
`onevent_DebugMe`, `onevent_Count`, and for each `onevent_Event<n>Enabled`,
`onevent_Event<n>Source`, `onevent_Event<n>Monitor`, `onevent_Event<n>WhereCount`,
`onevent_Event<n>Where<m>`, `onevent_Event<n>ActionCount`, `onevent_Event<n>Action<m>`; RLV:
`rlv_Enabled`; e-mail: `smtp_Enabled`, `smtp_Host`, `smtp_Port`, `smtp_useSSL` (`true`:
`FERRO_SMTP_TLS=tls`), `smtp_User`, `smtp_Token`, `smtp_MailReplyAddress`,
`smtp_AllowCommandSendMail`, `smtp_AllowEmailAsReplyTarget`, `smtp_UseAllowedRecivers` with
`smtp_AllowedReciversCSV`, `smtp_AllowSendAlertStatus`, `smtp_AllowSendLoginNotice`,
`smtp_SendAlertsAndLoginsTo`; and the relays:
`relay_count`, `relay_UseShortEncoder`, `relay_<n>_enabled`, `relay_<n>_extended`,
`relay_<n>_source_type`, `relay_<n>_source_option`, `relay_<n>_target_type`,
`relay_<n>_target_option`, `relay_<n>_RelayConfig`; and the custom commands: `customcommands_count`,
`customcommands_<n>_trigger`, `customcommands_<n>_args`, `customcommands_<n>_steps`,
`customcommands_<n>_commands_<m>`.

### Service control

With `FERRO_ALLOW_SERVICE_CONTROL=true`, SecondBot's service commands stop, start and restart the
bot's optional services while it runs, and change their settings: `EnableAndStartService`,
`StopService`, `RestartService` (a service name) and `ChangeConfigValue` (a service, a setting and
its value). Each answers `ok`, or SecondBot's "Service control is not enabled by config" and
"Unknown service". They count as commands that change things (the rate limit), and a command grant
(SetPermFlag) does not reach them.

| Service | What it is | Settings ChangeConfigValue takes |
|---|---|---|
| `RelayService` | the relays | `count`, `UseShortEncoder`, `<n>_enabled`, `<n>_extended`, `<n>_source_type`, `<n>_source_option`, `<n>_target_type`, `<n>_target_option`, `<n>_RelayConfig` |
| `HomeboundService` | homebound | `Enabled`, `HomeSimSlUrl`, `BackupSimSLUrl`, `ReturnToHomeSimAfterMins`, `AtHomeSeekLocation`, `AtHomeAutoSitUuid` |
| `InteractionService` | the answers to offers (stopped, offers are left unanswered) | `AcceptTeleports`, `TeleportRequestLevel`, `AcceptGroupInvites`, `GroupInviteLevel`, `AcceptInventory`, `InventoryTransferLevel`, `AcceptFriendRequests`, `FriendRequestLevel`, `EnableJsonOutputEvents`, `JsonOutputEventsTarget` |
| `DialogService` | script dialogs kept and relayed (a start sets the relay targets from the settings) | `RelayChannel`, `RelayAvatar`, `RelayObjectOwnerOnly`, `RelayHttpurl` |
| `DiscordService` | the bot on Discord | `Enabled`, `ServerID`, `AllowDiscordCommands`, `InteractionEnabled`, `InteractionCommandName`, `InteractionHttpTarget`, `InteractionChannelNumber`, `hideChatterName`, `ClearChatOnConnect` |
| `ChatLogService` | Ferrobot's chat and IM log lines | none |

The settings are SecondBot's, named without their file's prefix (`count` for `relay_count`). A
changed value wins over the environment until the bot stops, and takes effect at the service's next
start: `RestartService` (or `EnableAndStartService`, which also sets its `Enabled` setting to true).
An empty value unsets it: its default applies, not the environment's. Settings that hold secrets
(tokens, passwords, keys) are refused ("Secret values can not be changed by command"); a setting the
service does not read answers "Unknown config name", a value it cannot read "Invalid config value".
The answers never carry the arguments, the command history keeps the service and the setting but not
the value, and a command by IM is logged by its name alone.

Never stoppable nor changed by command, so that nobody locks the operator out or changes whose
commands the bot takes (its masters, the shared secret, `FERRO_DISCORD_MASTERS`): the login and the session (`BotClientService`), the HTTP API (`HttpService`): both
"Unable to manage selected service"; the commands and their settings, masters and shared secret
among them (`CommandsService`, unknown to the service commands as in SecondBot), the commands by IM,
the chat history, the health file. SecondBot stops BotClientService and HttpService when asked.

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
POST to `<url>/Relay/Service`), `rabbit` (a RabbitMQ queue, by name; RabbitMQ on), `webhook` (a
JSON `{"content": …}` POST, as a Discord webhook takes it). Relayed lines are marked `{Relay}` and never relayed again. The `FERRO_*` name
wins when both are set. `http_DisableCommandValidation` is ignored: commands are always signed.

### AI replies

SecondBot's ChatGPT service, text only: the bot answers what it hears with what an
OpenAI-compatible chat completions API writes. Off unless `FERRO_CHATGPT=true`, and then only
where it is switched on: local chat said by avatars (`FERRO_CHATGPT_LOCALCHAT`), IMs
(`FERRO_CHATGPT_IM`, friends only with `FERRO_CHATGPT_IM_FRIENDS_ONLY`), one group's chat
(`FERRO_CHATGPT_GROUP` and its key). Local chat is one conversation, each avatar's IMs one, the
group one; each keeps its last lines and a cooldown, and is forgotten after a quiet spell. The
reply is only said, where the question came from (cut to two chat lines): it never runs a
command, and an IM that is a command for the bot is never sent to the API. Any provider that
speaks OpenAI's API works: set `FERRO_CHATGPT_PROVIDER` to its address (`https://…/api/` for
OpenRouter, `http://host:11434` for a local Ollama) and `FERRO_CHATGPT_MODEL` to its name for
the model. Each question costs a request: keep the cooldowns, and think twice before answering
local chat in a busy place. There is no voice (no audio library in the container) and no Redis
store (`chatgpt_UseRedis` is ignored).

### Events

What happens around the bot, as SecondBot's events service tells it: a JSON object
`{"EventName": …, "Data": {…}, "Hash": …, "Unixtime": …, "BotUUID": …}` said on
`FERRO_EVENTS_CHANNEL`, sent by IM to `FERRO_EVENTS_AVATAR` and posted to `FERRO_EVENTS_HTTP` +
`/Event/Service` (form fields `token`, `unixtime`, `method=Dialog`, `action=Relay`, `botname`,
`event`; `token` = sha1(unixtime + `EventService` + the bot's key + event + the shared secret)).
`Hash` = sha1(BotUUID + EventName + each value of Data in order + Unixtime +
`FERRO_EVENTS_SECRET`). The events and their Data:

- `GroupMembership`: `uuid`, `name`, `action` (`join`, `leave`), for `FERRO_EVENTS_GROUP`;
- `GuestTracking`: `uuid`, `name`, `action` (`enter`, `exit`), the avatars of the region while the
  bot stands on the parcel named;
- `ChangedSim`: `lastsim`, `newsim`; `ChangedParcel`: `oldparcel`, `newparcel` (the first parcel
  after the start comes with an empty `oldparcel`);
- `AlertMessage`: `Message`, `NotificationId` (null without one);
- `StatusMessage`: `Message`, Ferrobot's state: `In world: <bot> in <region>`,
  `Disconnected: <why>` (SecondBot lists its services instead);
- `MoneyEvent`: `fromuuid`, `touuid`, `amount`, `balance`, `fromname`, `toname`,
  `transactionid`, for every balance the grid sends (a payment, and the balance at each login).

### Commands on events

Commands the bot runs by itself when something happens, as SecondBot's TriggerOnEventService.
Each event has a source, what it watches (`MONITOR`), conditions (`WHERE_1`, `WHERE_2`…, all must
hold) and actions (`ACTION_1`, `ACTION_2`…, in order):

| Source | Monitor | Its own values |
|---|---|---|
| `AvatarIm` | `Any`, or an avatar's key | `message`, `avatarname`, `avataruuid` |
| `SimAlert` | | `alertmessage` |
| `BalanceUpdate` | | `funds` |
| `GroupMemberJoin`, `GroupMemberLeave` | a group's key (one the bot is in) | `groupuuid`, `groupname`, `avataruuid` |
| `ChangeSim` | | `oldsimname`, `newsimname` |
| `GuestJoins`, `GuestLeaves` | `Region Name=#=Parcel Name` (the bot and the avatar there) | `avataruuid` |
| `Clock` (every minute) | | |

Every event also has `eventype`, `avatarname`, `avatarparcel`, `avatarx`, `avatary`, `avatarz`,
`avatardistance`, `botsim`, `botparcel`, `botx`, `boty`, `botz`, `clockhour`, `clockmin` and
`dayofweek` (0 is Sunday; the clock is UTC). A value is written `[name]`. A condition is
`left{OP}right` with `IS`, `NOT`, `CONTAINS`, `MISSING`, `LESSTHAN`, `MORETHAN`, `IN_GROUP`,
`NOT_IN_GROUP` (an avatar's key, a group's key), `IS_UUID`, `IS_EVEN`, `DIVISIBLE` and `LOCKOUT`
(`[avataruuid]{LOCKOUT}false`). An action is a command in the text form of commands by IM
(`Say|||0~#~Hello [avatarname]`, `IM|||[avataruuid]~#~Welcome#|#reply target`) or as JSON, or
`lockout=[avataruuid]+5` (that key locked out 5 minutes). For example, greeting newcomers:

```
FERRO_ON_EVENT=true
FERRO_ON_EVENT_1_SOURCE=GuestJoins
FERRO_ON_EVENT_1_MONITOR=My Region=#=My Shop
FERRO_ON_EVENT_1_WHERE_1=[avataruuid]{LOCKOUT}false
FERRO_ON_EVENT_1_ACTION_1=IM|||[avataruuid]~#~Welcome to the shop, [avatarname]!
FERRO_ON_EVENT_1_ACTION_2=lockout=[avataruuid]+60
```

The actions come from the settings only, and run as commands under the rate limit. The values
come from the grid (an IM's text, a name): they only ever fill the arguments they stand in, so a
message holding `~#~` or `#|#` cannot add an argument or a reply target, and the command's name
never takes one. SecondBot puts them into the whole action's text before reading it. Other
differences: `IS_UUID`, `IS_EVEN` and `DIVISIBLE` work and a condition without an operator
leaves its event out (SecondBot drops the condition); an `AvatarIm` monitor that is an avatar's
key watches that avatar only; `botx`, `boty`, `botz` are the bot's place; a guest who leaves
is placed where it was last seen, so `GuestLeaves` runs (SecondBot's never does).

### RLV

RestrainedLove, as SecondBot's RLV service: an object says `@` commands to its owner
(`llOwnerSay("@sit:<key>=force")`), and the bot keeps the restrictions (`=n`, `=y`, `@clear`),
answers the queries on the channel given, and does what is forced. Only objects owned by the bot
or by a master are heard (SecondBot hears any; the grid already sends an OwnerSay only to its
owner). An attachment that says `@detach=n` keeps itself on through RLV's own detaches; the
bot's commands (`Detach`, `SwapOutfit`) still take it off, as SecondBot's do.

- Queries (`=<channel>`, above 0): `version`, `versionnew`, `versionnum`, `getstatus`,
  `getstatusall` (`:<part>;<separator>`), `getgroup`, `getsitid`, `getoutfit[:<layer>]`,
  `getattach[:<point>]`, `getpath` (the asking attachment), `getpathnew:<attachment key>`,
  `getinv[:<folder>]`, `getinvworn[:<folder>]`, `findfolder:<words>`, `findfolders:<words>`
  (words joined by `&&`).
- Forced (`=force`): `sit[:<key>]` (the ground without one), `unsit`, `setrot:<radians>`,
  `tpto:<x>/<y>/<z>` (global) or `tpto:<region>/<x>/<y>/<z>`, `setgroup:<name>`,
  `detach[:<point or folder>]` and `remattach` (everything without an option), `detachall:<folder>`,
  `detachallthis`, `remoutfit:<layer>`, `attach`, `attachoverorreplace`, `attachover`, `attachall`,
  `attachallover` (`:<folder>`).
- Restrictions (`=n` / `=add`, `=y` / `=rem`): any behaviour is kept and reported by
  `getstatus`; the bot itself only obeys `detach` (as SecondBot).

Folders are paths under `#RLV` in the inventory's root (`Gear/Hats`), whatever their case.
The version answers are SecondBot's texts with Ferrobot's name:
`RestrainedLife viewer v1.23 (Ferrobot)`, `RestrainedLove viewer v1.23 (Ferrobot)`, `1230100`.

### E-mail

SecondBot's SMTP service, off unless `FERRO_SMTP` is on, each part off until allowed:
`SendEmail` (`to`, `subject`, `body`; answers `sending` once the server took the message),
a command's answer sent to an e-mail reply target (subject `<command>##commandreply`), and the
grid's alerts and each login mailed to `FERRO_SMTP_NOTIFY` ("Bot - Alert message", "Bot - Login
message"). Mail comes from `FERRO_SMTP_FROM`, one connection per message; a server that does not
answer costs a failed send (20 s a step, 60 s a message), never a stuck bot.

Any signed command or master can mail any address once `SendEmail` or the reply target is on:
set `FERRO_SMTP_ALLOWED` so that the bot cannot be used to mail anyone else. `SendEmail` counts
in `FERRO_RATE_LIMIT`. Neither the password nor an address is logged.

Unlike SecondBot's: STARTTLS is required unless `FERRO_SMTP_TLS=none` (MailKit falls back to a
clear connection, password included, when the server offers no STARTTLS); the login notice
follows `smtp_AllowSendLoginNotice` (SecondBot reads `smtp_AllowSendAlertStatus` for both); the
allowed list ignores spaces and case.

### Grants

What an avatar may have beyond the `FERRO_ACCEPT_*` levels, until the bot stops (a switch that is
off stays off). `AcceptNext` (stores `FriendRequest`, `InventoryOffer`, `GroupInvite`, `Teleport`)
and `SetPermFlag` with sticky `false` accept the avatar's next request of that kind, once;
`SetPermFlag` with sticky `true` (and `AddToAllowAnimations`, for animations) until taken back
(`SetPermFlag` with state `false`). SetPermFlag's flags: `friend`, `group` (found by the inviter's
name), `teleport`, `inventory`, `animation` (that avatar's objects may animate the bot, nothing
more), and `command`: the avatar's IMs run as commands without a signature, as a master's do,
except `SetPermFlag`, `AcceptNext` and `AddToAllowAnimations`, so that a grant cannot grow or pass
itself on (its objects still sign). SecondBot's SetPermFlag answers the same and does nothing.

### RabbitMQ

The bot on an AMQP 0-9-1 broker (RabbitMQ), as SecondBot's RabbitService. It publishes there:
`SendMessageToQ` and the relays' `rabbit` target, each message to a queue by its name (declared
first, durable, as SecondBot does). It reads SecondBot's four queues, JSON messages with
SecondBot's field names:

| Queue | Message | What the bot does |
|---|---|---|
| `notecards` | `{"Name", "Subject", "Content", "UUID"}` | Makes the notecard and gives it to that avatar |
| `commands` | `{"CommandName", "CommandArgs": […]}` | Runs the command (no answer is sent) |
| `ims` | `{"UUID", "Message"}` | The `IM` command (an avatar's key or name) |
| `groupims` | `{"UUID", "Message"}` | The `Groupchat` command (a group's key) |

Unlike SecondBot, **every message read must be signed** with the shared secret, as a command by
IM: a `Signing` field = sha1(name + arguments joined with `~#~` + secret), where the name and
arguments are the command's for `commands`, `IM` with UUID and Message for `ims`, `Groupchat`
with UUID and Message for `groupims`, `Notecard` with Name, Subject, Content and UUID for
`notecards`. With `FERRO_ENFORCE_TIME_WINDOW` on, a `Unixtime` field goes in the signature after
the arguments and the message must be recent. Unsigned messages are dropped (and logged); without
a shared secret the queues are not read at all. Each message is taken once, never run twice.

The broker may be down: the bot logs in and works as ever, publishing fails at once ("Connection
is not open"), and it connects again, waiting longer each time (up to a minute).

### Custom commands

Commands of your own, made of the bot's commands (SecondBot's custom commands). Each step is a
command in the text form of commands by IM, `Command|||arg~#~arg`, optionally with a reply target
for its answer (`#|#7`, an avatar, an http(s) address), or in the JSON form; `[C_ARG_1]`,
`[C_ARG_2]`… stand for the caller's arguments, anywhere in a step:

```
FERRO_CUSTOM_COMMAND_1_TRIGGER=welcome
FERRO_CUSTOM_COMMAND_1_ARGS=1
FERRO_CUSTOM_COMMAND_1_STEP_1=GroupInvite|||<group key>~#~[C_ARG_1]~#~everyone
FERRO_CUSTOM_COMMAND_1_STEP_2=IM|||[C_ARG_1]~#~Welcome to the group!
```

`welcome` is then asked for like any command, by the HTTP API (signed with its own name and
arguments), by IM, on Discord, with at least `ARGS` arguments ("Incorrect number of args"); its
name is matched as written, case included, and a built-in command's name cannot be taken. The steps
run one after the other, each as if it had been asked for on its own, by the same caller: its
argument count, its rate limit, and the caller's rights (an avatar with a `command` grant still
cannot reach `SetPermFlag`, `AcceptNext` or `AddToAllowAnimations` through one). A step cannot run
another custom command. The answer is `welcome: ran with 2 commands issued` (steps that failed are
not counted), or `all custom commands have failed for: welcome`; each step's answer is in the log
and the command history. An argument is put in as plain text: one holding `~#~` or `#|#` is still
one argument, never more arguments or a reply target. With SecondBot's names (`customcommands_count`
commands, `customcommands_<n>_steps` steps each), the same commands; a missing trigger or step, a
trigger given twice or a built-in name stop the bot at start-up rather than running `Logoff` as
SecondBot does.

### SecondBot's HTTP API

`POST /api/Run` with the form fields `commandName`, `args` (joined with `~#~`), `unixtime` and
`signing` = sha1(command + args + unixtime + shared secret), as SecondBot's. The answers are
SecondBot's: `{"Key": true, "Value": "{\"command\":…,\"reply\":…,\"status\":true}"}`, or a plain-text
refusal. The commands so far are the ones BB's SL group sync sends: `SimName`, `Name`, `Name2Key`,
`GetGroupList`, `GetGroupRoles`, `GetGroupMembers`, `GetGroupRoleMembers`, `IsGroupMember`,
`GroupInvite`, `GroupEject`, `GroupAddRole`, `GroupRemoveRole`; the other group commands:
`Groupnotice`, `GroupnoticeWithAttachment`, `GetGroupBanList`, `GroupBan`, `GetGroupTitles`,
`GroupActiveTitle`, `GroupActiveGroup`, `GroupJoin`, `GroupLeave`, `GetGroupMemberRoles`,
`ForceLoadGroups`, `GroupCreate` (L$100, a Funds command); `RebakeAppearance`; the inventory: `InventoryFolders`,
`InventoryFoldersLimited`, `InventoryContents`, `SendItem`, `SendFolder`, `SendItemByPath`,
`SendFolderByPath`, `SendItemDirect`, `GetRealUUID`,
`CreateInventoryFolder`, `RenameInventory`, `DeleteInventoryItem` (to the Trash),
`DeleteInventoryFolder`, `MoveInventoryItem`, `MoveInventoryFolder`, `InventoryPurgeNotecards`
(the root's "Notecards" folder: notecards 31 days old or more, removed for good),
`InventoryContentsCatched` (the whole tree, fetched when asked: Ferrobot keeps no cache),
`InventoryPreloadCount`, `InventoryPreloadDone` (always "Ready": Ferrobot fetches on demand),
`SetInventoryUpdate` (a target hears of each item of a type that comes into the inventory);
`UploadMediaWave` (unlike SecondBot's, which encodes a WAV file from its own disk: an Ogg Vorbis
sound from an http(s) URL, mono, 44100 Hz, 30 s at most, 2 MiB at most, uploaded to the Sounds
folder; it costs what the grid asks for a sound, so it is a Funds command unless the login says
sounds are free);
`SetPermFlag` (grants an avatar's friendships, group invites, teleports, items, animations or
commands by IM, once or lasting: see "Grants" below); `AcceptNext`, `IsOnAcceptNext` (SecondBot's
one-time accept list); `ListSculptys` (owners and names asked of the region);
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
avatar's objects may animate the bot); profiles: `GetProfileAbout`, `GetProfileImage`, `GetProfilePicks`,
`SetProfileAbout`, `SetProfileImage`; objects: `RezObject`, `RezObjectOnPos`, `RezObjectParcelCenter`,
`AsyncRezObject`, `AsyncRezObjectOnPos`, `AsyncRezObjectParcelCenter`, `ClickObject`, `UnRezObject`,
`GetObjectsInParcel` (with each object's owner and name, asked of the region),
`TransferInventoryToObject`, `AsyncTransferInventoryToObject`; what it wears:
`Attach`, `Detach`, `SwapOutfit`, `SwapOutfitByPath`, `Outfit`; movement: `Fly`,
`SetAlwaysRun`, `Sit`, `Stand`, `AutoPilot`, `AutoPilotStop`, `RotateTo`, `RotateToFace`,
`RotateToFaceVector`, `HoverHeight`, and Ferrobot's own (SecondBot has none): `Walk` (direction
`forward`, `back`, `left` or `right`, and seconds up to 60), `WalkStop`, `Turn` (`left` or `right`,
and degrees), `Follow` (an avatar, by key or name, and the metres to keep from it, 0.5 to 20; until
it leaves the region), `FollowStop` (`AutoPilotStop` stops a walk and a follow too); `Hello`, `Version`, `UnixTimeNow`, `GetPosition`, `GoHome`, `Logoff`,
`GetLastCommands`, `ReadKeyValue`, `SetKeyValue`, `ClearKeyValue`, `SendEmail` (see "E-mail");
Ferrobot's own `BotStatus` (in world, region, logins, uptime, commands run and failed, e-mails
sent and failed); SecondBot's chat commands:
`LocalChatHistory`, `Say`, `Shout`, `Whisper`, `IM`, `chatwindows`, `listwithunread`,
`haveunreadims`, `getimchat`, `Groupchat`, `GroupchatHistory`, `GroupchatListAllUnreadGroups`,
`GroupchatGroupHasUnread`, `GroupchatAnyUnread`, `GroupchatClearAll`; Discord: `Discord_AddRole`,
`Discord_BanMember`, `Discord_GetMemberRoles`, `Discord_BulkClear_Messages`, `Discord_Dm_Member`,
`Discord_MembersList`, `Discord_MessageChannel`, `Discord_MuteMember`, `Discord_Role_GetSettings`,
`Discord_Role_UpdatePerms`, `Discord_RoleCreate`, `Discord_RoleList`, `Discord_RoleRemove`,
`Discord_TextChannels_List`, `Discord_VoiceJoin`, `Discord_VoiceLeave`; its dialog commands:
`DialogResponce`, `DialogRelayChatTarget`, `DialogRelayAvatarTarget`, `DialogRelayHttpTarget`;
RabbitMQ: `SendMessageToQ`;
the service control (see above): `EnableAndStartService`, `StopService`, `RestartService`,
`ChangeConfigValue`.

An empty argument cannot be sent in the form: `args` is split on `~#~` with the empty parts left
out, before the signature is checked, as SecondBot does (so `a~#~` is the one argument `a`, and a
signature made over `a~#~` is refused).

`POST /json` is SecondBot's other entry: the body is one command, as a JSON object
`{"cmd": "Name2Key", "args": ["Some Resident"], "unixtime": 1800000000, "signing": "…", "reply": "7"}`
(`reply` optional: a channel, an avatar, an http(s) address or an e-mail address that gets the
answer too), or in the text form
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
