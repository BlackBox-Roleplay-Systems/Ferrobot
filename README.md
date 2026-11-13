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
| `FERRO_MASTERS` | (none) | Avatars the bot obeys, comma-separated: `First Last` or usernames. Their teleport offers are accepted |
| `FERRO_SHARED_SECRET` | (none) | The secret that signs the HTTP API's commands. Without it, the API is off |
| `FERRO_HTTP` | `true` | SecondBot's HTTP API (`POST /api/Run`) |
| `FERRO_HTTP_PORT` | `80` | Its port |
| `FERRO_TIME_WINDOW` | `35` | Seconds a signed command stays valid |
| `FERRO_HEALTH_FILE` | `/tmp/ferrobot-health` | Where the bot writes its health, for `ferrobot healthcheck` |
| `RUST_LOG` | `info` | Log detail: `debug` shows every event, `trace` more |

SecondBot's names work too, so that a SecondBot container's settings can stay as they are:
`basic_Username`, `basic_Password`, `basic_LoginURI`, `commands_MastersCSV`,
`commands_SharedSecret`, `commands_TimeWindowSecs`, `http_Enabled`, `http_Port`. The `FERRO_*` name
wins when both are set. `http_DisableCommandValidation` is ignored: commands are always signed.

### SecondBot's HTTP API

`POST /api/Run` with the form fields `commandName`, `args` (joined with `~#~`), `unixtime` and
`signing` = sha1(command + args + unixtime + shared secret), as SecondBot's. The answers are
SecondBot's: `{"Key": true, "Value": "{\"command\":…,\"reply\":…,\"status\":true}"}`, or a plain-text
refusal. The commands so far are the ones BB's SL group sync sends: `SimName`, `Name`, `Name2Key`,
`GetGroupList`, `GetGroupRoles`, `GetGroupMembers`, `GetGroupRoleMembers`, `IsGroupMember`,
`GroupInvite`, `GroupEject`, `GroupAddRole`, `GroupRemoveRole`.

In Second Life, mark the account as a **scripted agent** (account settings on secondlife.com), as the grid's rules ask of bots.

`ferrobot healthcheck` (the image's Docker `HEALTHCHECK`) exits with 0 while the bot is online, or
offline for less than 10 minutes (a relog), and with 1 otherwise.

Exit codes: `0` logged out on request, `1` bad settings, `2` a login refusal retrying can't fix (wrong password, terms of service not accepted, two-factor authentication on).

## Build and run

The image is published for amd64 and arm64:

```
docker run -d --name ferrobot --restart unless-stopped \
  -e FERRO_USERNAME='Bot Name' -e FERRO_PASSWORD='…' ghcr.io/kanikabreed/ferrobot:latest
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
