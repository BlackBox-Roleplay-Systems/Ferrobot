# Roadmap

The plan for Ferrobot, from a bot that logs in to a full replacement for SecondBot. Each phase ends
with something the human can run and check. [TASKS.md](TASKS.md) tracks the steps; this file
changes only when the plan does.

## The goal

A Second Life bot in Rust, in a small Docker container, that:

1. **stays online** on its own, for weeks: it logs back in after a drop, a region restart or a grid
   problem, which SecondBot doesn't;
2. **manages Second Life groups** for websites such as Black Box: invites, ejects, roles, member
   lists, the commands BB's SL group sync already sends;
3. **answers SecondBot's HTTP API**, same commands, same signed requests, same replies, so that
   BB (`SlBot.php`) and SecondBot's other users can switch by changing an address;
4. then grows towards the rest of SecondBot: chat and IM relays, inventory, movement, parcels,
   estate.

## What it is built from

- **Aurora Viewer**, a Second Life viewer in Rust (GPL-3.0): the protocol code is ported from it
  and trimmed for a bot. It is the most up-to-date reference. LLSD and the message system are
  already in (`crates/llsd`, `crates/msg`); login, the circuit, capabilities and the event queue
  are ported.
- **LibreMetaverse**: how a headless client does what a viewer doesn't need to.
- **SecondBot**: the list of commands, their exact names and replies, its settings and behaviour.

AGENTS.md section 2 says how each is used.

---

## Phase 0 - Foundations ✅

Workspace, GPL-3.0, LLSD and message crates from Aurora, Dockerfile, and a first session: log in,
arrive in the region, keep the connection alive, log chat and IMs, log out on `docker stop`, log in
again after a drop.

## Phase 1 - First login, and a grid of our own for tests

**Goal:** the bot is proven on the real grid once, and every later change can be tested without it.

- **First login on the grid** (the human): the bot arrives, chat and IMs show in the log, Ctrl-C
  logs it out, a dropped connection logs it back in. Fix what that shows.
- **Docker**: build and run the image; `docker stop` logs out; note its size, and keep it small
  (a slim or distroless base, nothing but the binary and the CA certificates).
- **The test grid** (`crates/testgrid`): a fake login server (XML-RPC) and a fake region (UDP
  circuit, seed capability, event queue) on localhost. Tests run the real bot against it: login,
  arrival, pings, logout, a kick, a lost connection and the relog. Each later phase adds what it
  needs (group replies, chat…).
- **CI on GitHub**: fmt, clippy, the tests, and a Docker build, on every PR.
- **Agent tooling**: `scripts/check.sh`, `new-task.sh`, `end-task.sh`.

**Done when:** the human has seen the bot in world, and `check.sh` runs a full login against the
test grid.

## Phase 2 - The client becomes a library

**Goal:** a clean core that every feature is built on, before the features pile up.

- Split `crates/client` (ferro-client) out of the binary: the network task, requests with replies
  (`mpsc` + `oneshot`, timeouts), events on a `broadcast` channel (ARCHITECTURE.md).
- **Regions**: neighbours (EnableSimulator), crossings (CrossedRegion), teleports (TeleportRequest,
  lures, TeleportFinish, EstablishAgentCommunication), region restarts. From Aurora's session.
- **The bot's own state**: AgentDataUpdate, its groups (AgentGroupDataUpdate), position, home.
- **Names**: key to name and back, display names (GetDisplayNames, UUIDNameRequest,
  AvatarPickerRequest), cached.
- **Health**: what the API's health check and the Docker `HEALTHCHECK` read (logged in, region,
  last packet).

**Done when:** the bot can be teleported by its master and keeps working, and the existing session
code runs on the new core with its tests.

## Phase 3 - Groups

**Goal:** everything BB's SL group sync needs, done better than the bot it replaces.

Port `aurora-net/src/groups.rs` and its session handlers:

- the bot's groups, with names; the active group;
- a group's roles (GroupRoleDataRequest) and titles;
- members: the GroupMemberData capability, with the UDP request as the fallback, fetched on demand
  (not only at login, as SecondBot does) and cached with a time;
- who holds a role (GroupRoleMembersRequest);
- invite to a group with a role (InviteGroupRequest), eject (EjectGroupMemberRequest), give and
  take a role (GroupRoleChanges);
- is someone a member;
- accepting group invites, by the same rules as SecondBot's interaction settings (masters, levels);
- group chat: join a group's session, send, receive (for relays, phase 5).

**Done when:** on the grid, the human checks each of these on a test group, and the test grid
covers each reply.

## Phase 4 - SecondBot's HTTP API

**Goal:** BB talks to Ferrobot without a line changed.

- `POST /api/Run`: the signed request (sha1 of command, args, unixtime and the shared secret; 35 s
  window; empty arguments dropped), the `{Key, Value}` reply, the plain-text refusals ("Command
  request rejected", "No bot connected yet"), "Unknown command".
- **BB's commands first**, answering exactly as SecondBot's do: `SimName`, `Name`, `Name2Key`,
  `GetGroupList`, `GetGroupRoles`, `GetGroupMembers`, `GetGroupRoleMembers`, `IsGroupMember`,
  `GroupInvite`, `GroupEject`, `GroupAddRole`, `GroupRemoveRole`.
- SecondBot's setting names accepted (`http_Port`, `commands_SharedSecret`, `basic_Username`…) next
  to `FERRO_*`, so a SecondBot container's settings work.
- Test against BB: BB's own bot test (the stub-and-tests harness used to build the SL group sync)
  pointed at Ferrobot on the test grid, then the human's dev site with Ferrobot on the grid.

**Done when:** BB's dev site runs its SL group sync, report and fixes on Ferrobot.

## Phase 5 - Talking and listening

- Send local chat, IMs and group chat; read the history (SecondBot's `Chat` commands).
- **Masters and interactions**: SecondBot's rules for friend requests, teleport offers, inventory
  offers and group invites (accept from masters, or by level).
- **Commands in chat or IM** from masters, signed or not as SecondBot allows.
- Script dialogs: read and answer (SecondBot's `Dialogs`).
- **Relays**: chat, IMs and group chat to a webhook (Discord and plain HTTP), and back.
- **Homebound**: go home, or to a set place, after a time away (SecondBot's homebound service).

## Phase 6 - Looking like someone, and inventory

- **Appearance**: the bot wears its outfit (the current outfit folder) and the region bakes it
  (UpdateAvatarAppearance), so it is not a cloud; hover height.
- **Inventory**: fetch folders and items (AIS / FetchInventoryDescendents2), give items, accept
  items, create and send notecards (SecondBot's `Inventory` and `Notecard` commands).

## Phase 7 - The rest of SecondBot

In the order the human needs them: movement (walk, fly, sit, stand, autopilot, teleport), avatars
(profiles, online status), friends, animations, parcels, estate, funds. Each command as SecondBot
has it.

## Phase 8 - Running it for real

- A health and status endpoint; metrics (logged in time, relogs, commands, failures).
- Logs that help: one line per command with its result; no secrets.
- Rate limits on commands that change things.
- A published image (GitHub Container Registry) with versioned tags, built by CI.
- Several bots: one container each; nothing shared.
- Keeping up with Aurora: its protocol fixes brought in regularly (AGENTS.md section 2).

---

## Decisions to make along the way

| When | Decision |
|---|---|
| Phase 1 | Make the repository public, or keep it private for now |
| Phase 1 | Publish the image (GitHub Container Registry), or build it on the server |
| Phase 4 | Keep SecondBot's API forever, or add a cleaner JSON API next to it |
| Phase 5 | Which relays matter (Discord, webhooks, RabbitMQ, e-mail) |
| Phase 7 | Which of SecondBot's ~200 commands are worth porting |

## Risks

- **Grid rules**: a bot account must be marked as a scripted agent; behaviour that looks like
  abuse (mass invites, spam) gets accounts banned. Rate limits from phase 3 on.
- **Two-factor authentication** can't be answered by a bot: bot accounts must not have it on.
- **Protocol changes** on the grid: Aurora follows them; bring its fixes in.
- **Testing on the grid is the human's job** (AGENTS.md rule 1): the test grid carries everything
  else, so it has to stay faithful. Every surprise the human sees on the grid becomes a test-grid
  case.
