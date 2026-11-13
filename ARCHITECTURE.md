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
    src/events.rs          what the client reports (Event, chat, IMs)
    src/state.rs           what it keeps: who and where the bot is, its groups, its health
    src/names.rs           key <-> name cache
    src/region_restart.rs  region restart notices (from Aurora)
  ferrobot/   ferrobot     the bot
    src/main.rs            settings, signals, the login / relog loop, exit codes
    src/config.rs          FERRO_* environment variables
    src/services/          one service per job, on the client's events (chat_log, group_invites, health, lures, restart)
    tests/testgrid.rs      the real bot, run as a process against the test grid
  testgrid/   ferro-testgrid  a fake grid on localhost, for tests (ROADMAP phase 1)
    src/lib.rs             TestGrid: start, what the bot was seen doing, what the grid does to it
    src/login.rs           login_to_simulator, answered or refused as the grid does
    src/region.rs          the region's UDP side: circuits, handshake, arrival, pings, logout, kicks
    src/caps.rs            the seed capability and the held EventQueueGet poll
    src/http.rs, xmlrpc.rs a minimal HTTP/1.1 server and the grid's side of XML-RPC
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

### The container

Multi-stage `Dockerfile`: built on `rust:*-slim`, run on `debian:*-slim` as a non-root user, with
only `ca-certificates`. Settings by environment. `docker stop` (SIGTERM) logs the bot out.
