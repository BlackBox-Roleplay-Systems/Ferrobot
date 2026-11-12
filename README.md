# Ferrobot

A Second Life bot written in Rust, in the spirit of [SecondBot](https://github.com/Madpeterz/SecondBot) (.NET), run as a Docker container.

What it does so far: logs in, arrives in its region, stays connected (pings, agent updates, the event queue), logs chat and IMs it receives, logs out cleanly on `docker stop`, and logs in again by itself when the connection is lost.

## Settings

All settings come from environment variables.

| Variable | Default | What it is |
|---|---|---|
| `FERRO_USERNAME` | (required) | The bot's account: `First Last` or a username |
| `FERRO_PASSWORD` | (required) | Its password |
| `FERRO_LOGIN_URI` | Second Life | Another grid's login address |
| `FERRO_START` | `last` | `last`, `home`, or `Region Name/x/y/z` |
| `FERRO_RELOG` | `true` | Log in again after the connection is lost |
| `FERRO_RELOG_DELAY` | `60` | Seconds to wait before logging in again |
| `FERRO_HEALTH_FILE` | `/tmp/ferrobot-health` | Where the bot writes its health, for `ferrobot healthcheck` |
| `RUST_LOG` | `info` | Log detail: `debug` shows every event, `trace` more |

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

## Plan and contributing

- [ROADMAP.md](ROADMAP.md) - the plan, phase by phase
- [TASKS.md](TASKS.md) - where each step stands
- [ARCHITECTURE.md](ARCHITECTURE.md) - how the code is laid out
- [AGENTS.md](AGENTS.md) - rules for AI agents working on Ferrobot
