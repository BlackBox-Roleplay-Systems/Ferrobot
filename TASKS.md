# Tasks

✅ done (verified on the grid when it depends on it) · 🟡 done, not yet verified on the grid ·
🔧 in progress · ⬜ to do. The plan behind them is [ROADMAP.md](ROADMAP.md).

## Phase 0 - Foundations

- ✅ Workspace, GPL-3.0-or-later, NOTICE.md
- ✅ `crates/llsd` and `crates/msg` from Aurora Viewer b8cf1f047
- 🟡 Login (XML-RPC), from aurora-net/src/login.rs: same-server redirects only, device ids derived
  from the account, permanent refusals (password, ToS, 2FA) end with exit code 2
- 🟡 Region session: UseCircuitCode, CompleteAgentMovement, AgentThrottle, RegionHandshakeReply
  (flags 0x7, as Aurora and LibreMetaverse), pings, AgentUpdate every second, seed capability,
  EventQueueGet
- 🟡 Chat and IMs received are logged
- 🟡 SIGTERM / Ctrl-C log out; a lost connection or a kick logs in again after `FERRO_RELOG_DELAY`
- 🟡 Staying power, as Aurora's session: the region is lost after 100 s of silence (not 60), the
  arrival waits 240 s and logs out before giving up, a lost event queue ends the session (relog),
  a closed queue no longer spins the session loop, and any end during a logout counts as logged out

## Phase 1 - First login, and a test grid

- ⬜ First login on the grid (human): arrival, chat and IM in the log, logout, relog
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

- ✅ `crates/client` (ferro-client): the protocol code moved out of the bot; `Client` (requests with
  replies and a timeout, a broadcast of events, the state) and `Network` (the network task, one
  session at a time); the bot's chat and IM log is now a service on the events
- ⬜ Neighbour regions, crossings, region restarts
- ⬜ Teleports: requests, lures from masters, TeleportFinish, EstablishAgentCommunication
- ⬜ The bot's state: AgentDataUpdate, AgentGroupDataUpdate, position, home
- ⬜ Names: key <-> name, display names, cache
- ⬜ Health state for the API and the Docker HEALTHCHECK

## Phase 3 - Groups

- ⬜ The bot's groups and the active group
- ⬜ Roles and titles
- ⬜ Members (GroupMemberData capability, UDP fallback), on demand, cached
- ⬜ Role members
- ⬜ Invite with a role, eject, give and take a role
- ⬜ Is someone a member
- ⬜ Accept group invites by SecondBot's interaction rules
- ⬜ Group chat sessions

## Phase 4 - SecondBot's HTTP API

- ⬜ `/api/Run`: signature, 35 s window, replies and refusals as SecondBot
- ⬜ BB's commands: SimName, Name, Name2Key, GetGroupList, GetGroupRoles, GetGroupMembers,
  GetGroupRoleMembers, IsGroupMember, GroupInvite, GroupEject, GroupAddRole, GroupRemoveRole
- ⬜ SecondBot setting names accepted next to FERRO_*
- ⬜ BB's bot tests pass against Ferrobot on the test grid
- ⬜ BB dev site runs its SL group sync on Ferrobot (human)

## Phase 5 - Talking and listening

- ⬜ Send chat, IMs, group chat; history commands
- ⬜ Masters and interactions (friends, teleports, inventory, group invites)
- ⬜ Commands from masters in chat / IM
- ⬜ Script dialogs
- ⬜ Relays: Discord, webhooks
- ⬜ Homebound

## Phase 6 - Appearance and inventory

- ⬜ Current outfit and server-side baking; hover height
- ⬜ Inventory fetch, give, accept
- ⬜ Notecards: create and send

## Phase 7 - The rest of SecondBot

- ⬜ Movement, avatars, friends, animations, parcels, estate, funds (order set by the human)

## Phase 8 - Running it for real

- ⬜ Status endpoint and metrics
- ⬜ Rate limits on commands that change things
- 🟡 Published image with versioned tags (done in phase 1, see there)
