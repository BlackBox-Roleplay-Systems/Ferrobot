//! SecondBot's HTTP API: `POST /api/Run`, as its HttpService answers it (HttpWebBot.Run), so that
//! BB's SlBot.php and SecondBot's other users only change the address.
//!
//! The form fields `commandName`, `args` (joined with `~#~`), `unixtime` and `signing`; plain-text
//! refusals ("Invaild unixtime was sent", "No bot connected yet", "Command request rejected"),
//! else the JSON of CommandsService.RunCommand's KeyValuePair: `{"Key": bool, "Value": text}`.
//! Everything goes out as JSON with status 200, as SecondBot's SerializationCallback writes it.
//!
//! `POST /json`, as HttpService.JsonInput: the body is one command in SignedCommand's own form
//! (a JSON object `{cmd, args, signing, unixtime, reply}`, or the text form commands by IM use),
//! the answer to a reply target as well when it names one. Whatever it answers, the refusals
//! included, goes out as a JSON string: EmbedIO's SendDataAsync serialises SecondBot's text once
//! more. Deliberate difference: the signature always carries the time and a command older than
//! the window is refused, as on `/api/Run` (SecondBot follows EnforceTimeWindow there, off by
//! default, so a signed body could be sent again forever).
//!
//! The API only runs with a shared secret: every command is signed (AGENTS.md section 5).
//!
//! Behind a Cloudflare Tunnel (or any proxy): a command never takes longer than
//! [`COMMAND_DEADLINE`], under Cloudflare's 100 s origin timeout, so that a slow grid gives a
//! failure the caller can read rather than the proxy's error page; `GET /health` answers 200 while
//! the bot is in world and 503 otherwise, with nothing else, for uptime checks.

pub mod signing;

use crate::commands::{self, Ctx};
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Form, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// The longest a command may run: under Cloudflare's 100 s wait for an origin's answer.
pub const COMMAND_DEADLINE: Duration = Duration::from_secs(90);

struct Api {
    ctx: Ctx,
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

/// A command, cut short at [`COMMAND_DEADLINE`].
pub async fn run_command(ctx: &Ctx, command: &str, args: &[String]) -> (bool, String) {
    match tokio::time::timeout(COMMAND_DEADLINE, commands::run(ctx, command, args)).await {
        Ok(answer) => answer,
        Err(_) => (false, "No reply from the grid [Retry later]".to_owned()),
    }
}

/// Whether a command's answer says it was done.
fn done(ran: bool, value: &str) -> &'static str {
    if ran && value.contains(r#""status":true"#) {
        "done"
    } else {
        "failed"
    }
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
    if !api.ctx.client.health().online {
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
    let (ran, value) = run_command(&api.ctx, &command, &args).await;
    log::info!(
        "api: {command} ({} args): {}",
        args.len(),
        done(ran, &value)
    );
    json(serde_json::to_string(&KeyValue { key: ran, value }).unwrap_or_default())
}

/// `POST /json`: SecondBot's text, sent as a JSON string.
async fn json_input(State(api): State<Arc<Api>>, body: Bytes) -> Response {
    json(swan_string(&json_answer(&api, &body).await))
}

/// HttpService.JsonInput's text: a refusal, or RunCommand's `{Key, Value}`.
async fn json_answer(api: &Api, body: &[u8]) -> String {
    if !api.ctx.client.health().online {
        return "No bot connected yet".into();
    }
    if body.is_empty() {
        return "No body sent".into();
    }
    // GetRequestBodyAsStringAsync: what is not UTF-8 becomes U+FFFD
    let text = String::from_utf8_lossy(body);
    let accepted = signing::unpack(&text).filter(|u| {
        signing::accepted(
            &u.command,
            &u.args,
            u.unixtime,
            u.signing.as_deref(),
            &api.secret,
            api.window,
            now(),
        )
    });
    let Some(u) = accepted else {
        log::warn!("api /json: refused: malformed, bad signature or too old");
        return "Command request rejected".into();
    };
    let (ran, value) = run_command(&api.ctx, &u.command, &u.args).await;
    log::info!(
        "api /json: {} ({} args): {}",
        u.command,
        u.args.len(),
        done(ran, &value)
    );
    // RunCommand answers an unknown command before it looks at the reply target
    if let Some(target) = u.reply.filter(|_| ran || value != "Unknown command") {
        let (ctx, value, command) = (api.ctx.clone(), value.clone(), u.command.clone());
        tokio::spawn(async move {
            crate::smart_reply::send(&ctx, &target, &value, &command).await;
        });
    }
    serde_json::to_string(&KeyValue { key: ran, value }).unwrap_or_default()
}

/// A text as a JSON string, as Swan's serializer writes it (Json.Serializer.Escape, under
/// EmbedIO's default response serializer): `\`, `"` and `/` escaped, \b \t \n \f \r, the other
/// control characters as `\u00XX`, everything else as it is.
fn swan_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' | '"' | '/' => {
                out.push('\\');
                out.push(c);
            }
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if c < ' ' => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `GET /health`: in world or not, and nothing more (no secret needed, nothing to learn).
async fn health(State(api): State<Arc<Api>>) -> Response {
    if api.ctx.client.health().online {
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
pub async fn serve(ctx: Ctx, bind: std::net::IpAddr, port: u16, secret: String, window: u64) {
    let api = Arc::new(Api {
        ctx,
        secret,
        window,
    });
    let app = Router::new()
        .route("/api/Run", post(run))
        // SecondBot's /json module takes POST only; other methods reach its catch-all
        .route("/json", post(json_input).fallback(other))
        .route("/health", axum::routing::get(health))
        .fallback(other)
        .with_state(api);
    let listener = match tokio::net::TcpListener::bind((bind, port)).await {
        Ok(l) => l,
        Err(e) => {
            log::error!("HTTP API: port {port}: {e}");
            return;
        }
    };
    log::info!("HTTP API on port {port} ({bind})");
    if let Err(e) = axum::serve(listener, app).await {
        log::error!("HTTP API: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_json_strings_as_swan_writes_them() {
        assert_eq!(
            swan_string("Command request rejected"),
            r#""Command request rejected""#
        );
        assert_eq!(
            swan_string(r#"{"Key":true,"Value":"{\"reply\":\"a/b\"}"}"#),
            r#""{\"Key\":true,\"Value\":\"{\\\"reply\\\":\\\"a\/b\\\"}\"}""#
        );
        assert_eq!(
            swan_string("tab\tline\n\u{1}\u{8}\u{c}\r é"),
            r#""tab\tline\n\u0001\b\f\r é""#
        );
        // any JSON reader gets the text back
        let text = "{\"a\":\"x/y\\z\u{1f}\"}";
        let back: String = serde_json::from_str(&swan_string(text)).expect("a JSON string");
        assert_eq!(back, text);
    }
}
