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
| `RUST_LOG` | `info` | Log detail: `debug` shows every event, `trace` more |

In Second Life, mark the account as a **scripted agent** (account settings on secondlife.com), as the grid's rules ask of bots.

Exit codes: `0` logged out on request, `1` bad settings, `2` a login refusal retrying can't fix (wrong password, terms of service not accepted, two-factor authentication on).

## Build and run

```
cargo build --release
docker build -t ferrobot .
docker run --rm -e FERRO_USERNAME='Bot Name' -e FERRO_PASSWORD='…' ferrobot
```

## Layout

- `crates/ferrobot` - the bot: login, the region circuit, capabilities and the event queue, the session
- `crates/llsd` - LLSD, Second Life's structured data (from Aurora Viewer)
- `crates/msg` - the UDP message system, generated from `message_template.msg` (from Aurora Viewer)

See `NOTICE.md` for what comes from where.
