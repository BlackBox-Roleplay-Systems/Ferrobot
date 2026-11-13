# Architecture

Ferrobot is a Cargo workspace: the Second Life client is a library (`ferro-client`), and the bot
(`ferrobot`) is its settings, its relog loop and its services on top of it.

## Today

```
crates/
  llsd/       ferro-llsd   LLSD values: XML, binary and notation codecs (from Aurora Viewer)
  msg/        ferro-msg    UDP message system: packet framing, zerocoding, typed messages generated
                           at build time from message_template.msg (from Aurora Viewer)
  client/     ferro-client the Second Life client, as a library: no settings, no HTTP API
    src/lib.rs             Client: requests with replies, events, state; Network: the network task
    src/login.rs           XML-RPC login (login_to_simulator), the home location
    src/xmlrpc.rs          the XML-RPC codec of the login
    src/circuit.rs         reliable UDP circuit: sequence numbers, acks, resends, duplicates
    src/caps.rs            the seed capability and the EventQueueGet long poll
    src/network/mod.rs     one session in one region: connect sequence, keep-alive, incoming
                           messages, requests, teleports, logout
    src/network/groups.rs  group requests and replies (from Aurora's session_groups.rs)
    src/groups.rs          group data: roles, members, pairs, titles (from Aurora's groups.rs)
    src/group_api.rs       the client's group requests, the member list cache
    src/inventory.rs       inventory data, fetch bodies and answers (from Aurora's inventory.rs)
    src/inventory_api.rs   folders, items, gifts, moves; network/inventory.rs sends them
    src/events.rs          what the client reports (Event, chat, IMs)
    src/state.rs           what it keeps: who and where the bot is, its groups, its health
    src/names.rs           key <-> name cache
    src/region_restart.rs  region restart notices (from Aurora)
    src/objects.rs         where objects and the bot stand, from the object updates; their owners
                           and names, from ObjectProperties, asked at Area Search's pace (from Aurora)
    src/transfer.rs        protected asset downloads (TransferRequest) and notecard text (from Aurora)
    src/mutes.rs           the mute list: its download by xfer and its file (from Aurora)
    src/parcels.rs         a parcel's record (ParcelProperties) and its update (from Aurora)
    src/estate.rs          the estate's info, access lists and changes (from Aurora)
    src/terrain.rs         a terrain RAW file and its upload by xfer (from Aurora)
    src/gesture.rs         gesture assets (from Aurora) and their playing
    src/profile.rs         avatar profiles (AgentProfile, AvatarPropertiesReply)
    src/outfit.rs          attachments and outfits, by the Current Outfit Folder's links
    src/walking.rs         keys held in the agent updates, following an avatar by the autopilot
  ferrobot/   ferrobot     the bot
    src/main.rs            settings, signals, the login / relog loop, exit codes
    src/config.rs          FERRO_* environment variables
    src/api/               SecondBot's HTTP API: POST /api/Run and /json, signatures (axum)
    src/commands/          SecondBot's commands, one module per area (info, avatars, groups, chat, movement…)
    src/masters.rs         who the masters are, by name or by key
    src/grants.rs          what avatars were granted beyond the settings: AcceptNext, SetPermFlag
    src/dialogs.rs         script dialogs: kept, relayed, answered (SecondBot's DialogService)
    src/store.rs           key-value store, command history, counts, rate limit
    src/history.rs         what the bot heard: local chat, IM and group windows (SecondBot's store)
    src/smart_reply.rs     where a command's answer goes when it names a reply target
    src/inventory_updates.rs  items that come into the inventory, told to SetInventoryUpdate's targets
    src/download.rs        files the commands fetch from a URL: http(s) only, bounded in size and time
    src/discord/           the bot on a Discord server (twilight): its channels, both ways
    src/rabbit/            RabbitMQ (amqprs): the connection task, publishing, the signed queues read
    src/services/          one service per job, on the client's events (chat_log, health, homebound, im_commands, interactions, relays, restart, script_permissions)
    tests/testgrid.rs      the real bot, run as a process against the test grid
  testgrid/   ferro-testgrid  a fake grid on localhost, for tests (ROADMAP phase 1)
    src/lib.rs             TestGrid: start, what the bot was seen doing, what the grid does to it
    src/login.rs           login_to_simulator, answered or refused as the grid does
    src/region.rs          the region's UDP side: circuits, handshake, arrival, pings, logout, kicks
    src/caps.rs            the seed capability and the held EventQueueGet poll
    src/http.rs, xmlrpc.rs a minimal HTTP/1.1 server (and files to download), the grid's side of
                           XML-RPC
```

One tokio runtime. `Network::session` owns the UDP socket and the circuit, and `select!`s over the
socket, a 100 ms tick, the client's requests, the event queue's channel and the shutdown signal.

## Target (ROADMAP phase 2 onwards)

```
crates/
  llsd/        ferro-llsd      unchanged
  msg/         ferro-msg       unchanged
  client/      ferro-client    the Second Life client, as a library, no settings or HTTP:
    login, circuit, caps        (moved from ferrobot)
    network.rs                  the network task: socket, circuits per region, event queues
    region.rs                   regions: handshake, neighbours, crossings, teleports
    agent.rs                    the bot's own state: position, groups, appearance
    names.rs                    key <-> name, display names, cached
    groups.rs                   groups, roles, members, invites, ejects, role changes, group chat
    chat.rs, im.rs              local chat, IMs, group sessions
    friends.rs, inventory.rs, appearance.rs, parcels.rs, estate.rs …
    events.rs                   everything the client reports (a broadcast channel)
  testgrid/    ferro-testgrid  a fake grid on localhost, for tests (each feature adds its replies)
  ferrobot/    ferrobot        the bot: settings, services, the HTTP API
    api/                        /api/Run, SecondBot-compatible signed commands
    commands/                   one module per command area, like SecondBot's Commands/
    services/                   masters and interactions, homebound, relays, health
```

### How the pieces talk

- **The network task** owns the UDP socket, one circuit per region, and the event queues. Nothing
  else touches them.
- **Requests** go to it on an `mpsc` channel; each carries a `oneshot` for its answer. A command
  such as "the members of this role" sends the request and awaits the reply with a timeout: the
  network task matches the grid's answer (by request id) to the waiting caller.
- **Events** (chat, IMs, invites, region changes, disconnections) go out on a `broadcast` channel.
  Services subscribe to what they need.
- **State** the client keeps (groups, names, regions) lives in the client, behind small read
  methods, so that the API answers from memory when it can, like SecondBot's data store.

### The HTTP API

`POST /api/Run` with `commandName`, `args` (joined with `~#~`), `unixtime`, `signing` =
sha1(command + args + unixtime + shared secret), refused when older than 35 s. Replies as SecondBot
does: `{"Key": bool, "Value": "<JSON text>"}`, the inner JSON being
`{command, args, reply, status, whyfailed}`. A refused request gets plain text.

`POST /json` takes one command in SignedCommand's own form (a JSON object or the text form of
commands by IM, unpacked by `api/signing.rs` for both), signed with the time; it answers the same
texts, each sent as a JSON string as SecondBot's EmbedIO module does.

### The container and the tunnel

`deploy/compose.yaml`: the bot and cloudflared on one network, the API published through a
Cloudflare Tunnel, no port on the host. `GET /health` for checks from outside.

### The image

Multi-stage `Dockerfile`: built on `rust:*-slim`, run on `debian:*-slim` as a non-root user, with
only `ca-certificates`. Settings by environment. `docker stop` (SIGTERM) logs the bot out.
