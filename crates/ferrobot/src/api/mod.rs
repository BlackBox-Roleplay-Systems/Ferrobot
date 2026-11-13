//! SecondBot's HTTP API: `POST /api/Run`, as its HttpService answers it (HttpWebBot.Run), so that
//! BB's SlBot.php and SecondBot's other users only change the address.
//!
//! The form fields `commandName`, `args` (joined with `~#~`), `unixtime` and `signing`; plain-text
//! refusals ("Invaild unixtime was sent", "No bot connected yet", "Command request rejected"),
//! else the JSON of CommandsService.RunCommand's KeyValuePair: `{"Key": bool, "Value": text}`.
//! Everything goes out as JSON with status 200, as SecondBot's SerializationCallback writes it.
//!
//! The API only runs with a shared secret: every command is signed (AGENTS.md section 5).
//!
//! Behind a Cloudflare Tunnel (or any proxy): a command never takes longer than
//! [`COMMAND_DEADLINE`], under Cloudflare's 100 s origin timeout, so that a slow grid gives a
//! failure the caller can read rather than the proxy's error page; `GET /health` answers 200 while
//! the bot is in world and 503 otherwise, with nothing else, for uptime checks.

pub mod signing;

use crate::commands;
use axum::Router;
use axum::extract::{Form, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use ferro_client::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// The longest a command may run: under Cloudflare's 100 s wait for an origin's answer.
pub const COMMAND_DEADLINE: Duration = Duration::from_secs(90);

struct Api {
    client: Client,
    secret: String,
    window: u64,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct RunForm {
    commandName: Option<String>,
    args: Option<String>,
    signing: Option<String>,
    unixtime: Option<String>,
}

/// CommandsService.RunCommand's answer, as System.Text.Json writes a KeyValuePair.
#[derive(Serialize)]
struct KeyValue {
    #[serde(rename = "Key")]
    key: bool,
    #[serde(rename = "Value")]
    value: String,
}

fn json(body: String) -> Response {
    ([(header::CONTENT_TYPE, "application/json")], body).into_response()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

async fn run(State(api): State<Arc<Api>>, Form(f): Form<RunForm>) -> Response {
    let args = signing::split_args(f.args.as_deref());
    let Some(unixtime) = f
        .unixtime
        .as_deref()
        .and_then(|t| t.trim().parse::<i64>().ok())
    else {
        return json("Invaild unixtime was sent".into());
    };
    if !api.client.health().online {
        return json("No bot connected yet".into());
    }
    let command = f.commandName.unwrap_or_default();
    if !signing::accepted(
        &command,
        &args,
        unixtime,
        f.signing.as_deref(),
        &api.secret,
        api.window,
        now(),
    ) {
        log::warn!("api: {command:?} refused: bad signature or too old");
        return json("Command request rejected".into());
    }
    let (ran, value) = match tokio::time::timeout(
        COMMAND_DEADLINE,
        commands::run(&api.client, &command, &args),
    )
    .await
    {
        Ok(answer) => answer,
        Err(_) => (false, "No reply from the grid [Retry later]".to_owned()),
    };
    let done = ran && value.contains(r#""status":true"#);
    log::info!(
        "api: {command} ({} args): {}",
        args.len(),
        if done { "done" } else { "failed" }
    );
    json(serde_json::to_string(&KeyValue { key: ran, value }).unwrap_or_default())
}

/// `GET /health`: in world or not, and nothing more (no secret needed, nothing to learn).
async fn health(State(api): State<Arc<Api>>) -> Response {
    if api.client.health().online {
        (axum::http::StatusCode::OK, "ok").into_response()
    } else {
        (axum::http::StatusCode::SERVICE_UNAVAILABLE, "offline").into_response()
    }
}

/// Anything else: SecondBot's catch-all module answers `{"Message": "Error"}`.
async fn other() -> Response {
    json(r#"{"Message":"Error"}"#.into())
}

/// Serve the API until the bot stops. A port already in use is logged, and the bot goes on
/// without its API, as SecondBot does.
pub async fn serve(client: Client, port: u16, secret: String, window: u64) {
    let api = Arc::new(Api {
        client,
        secret,
        window,
    });
    let app = Router::new()
        .route("/api/Run", post(run))
        .route("/health", axum::routing::get(health))
        .fallback(other)
        .with_state(api);
    let listener = match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
        Ok(l) => l,
        Err(e) => {
            log::error!("HTTP API: port {port}: {e}");
            return;
        }
    };
    log::info!("HTTP API on port {port}");
    if let Err(e) = axum::serve(listener, app).await {
        log::error!("HTTP API: {e}");
    }
}
