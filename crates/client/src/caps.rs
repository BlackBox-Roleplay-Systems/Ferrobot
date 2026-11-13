//! Capabilities: the seed request and the `EventQueueGet` long poll.
//!
//! Port of parts of aurora-net/src/caps.rs (Aurora Viewer, see NOTICE.md); the retry rules are
//! `lleventpoll.cpp`'s. Capability URLs are secrets of the session: never logged.

use ferro_llsd::{Llsd, from_xml, llsd_map, to_xml};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;

/// The capabilities the bot asks the seed for.
pub const REQUESTED_CAPS: &[&str] = &[
    "EventQueueGet",
    "AgentPreferences",
    "AgentProfile",
    "ChatSessionRequest",
    "FetchInventory2",
    "FetchInventoryDescendents2",
    "GetDisplayNames",
    "GroupAPIv1",
    "GroupMemberData",
    "ReadOfflineMsgs",
    "UpdateAvatarAppearance",
];

#[derive(Debug, thiserror::Error)]
pub enum CapsError {
    #[error("network error: {0}")]
    Http(String),
    #[error("status {0}")]
    Status(u16),
    #[error("malformed LLSD")]
    Malformed,
}

pub async fn post_llsd(http: &reqwest::Client, url: &str, body: &Llsd) -> Result<Llsd, CapsError> {
    let resp = http
        .post(url)
        .header("Content-Type", "application/llsd+xml")
        .header("Accept", "application/llsd+xml")
        .body(to_xml(body))
        .send()
        .await
        .map_err(|e| CapsError::Http(e.without_url().to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(CapsError::Status(status.as_u16()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| CapsError::Http(e.without_url().to_string()))?;
    from_xml(&bytes).map_err(|_| CapsError::Malformed)
}

pub async fn get_llsd(http: &reqwest::Client, url: &str) -> Result<Llsd, CapsError> {
    let resp = http
        .get(url)
        .header("Accept", "application/llsd+xml")
        .send()
        .await
        .map_err(|e| CapsError::Http(e.without_url().to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(CapsError::Status(status.as_u16()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| CapsError::Http(e.without_url().to_string()))?;
    from_xml(&bytes).map_err(|_| CapsError::Malformed)
}

/// Ask the seed for the capabilities, a few times (the region may not be ready at once).
pub async fn resolve_seed(
    http: &reqwest::Client,
    seed: &str,
) -> Result<HashMap<String, String>, CapsError> {
    let req = Llsd::Array(REQUESTED_CAPS.iter().map(|s| Llsd::from(*s)).collect());
    let mut last_err = CapsError::Status(0);
    for attempt in 1..=5u32 {
        let reply = match tokio::time::timeout(Duration::from_secs(30), post_llsd(http, seed, &req))
            .await
        {
            Ok(reply) => reply,
            Err(_) => Err(CapsError::Http("no reply in 30 s".into())),
        };
        match reply {
            Ok(v) => match v.as_map() {
                Some(m) => {
                    return Ok(m
                        .iter()
                        .map(|(k, val)| (k.clone(), val.to_string_value()))
                        .filter(|(_, url)| !url.is_empty())
                        .collect());
                }
                None => last_err = CapsError::Malformed,
            },
            Err(e) => last_err = e,
        }
        log::warn!("seed capability: {last_err} (attempt {attempt} of 5)");
        tokio::time::sleep(Duration::from_secs(u64::from(attempt) * 2)).await;
    }
    Err(last_err)
}

/// One event from the region's event queue.
#[derive(Debug, Clone)]
pub struct EqEvent {
    pub message: String,
    pub body: Llsd,
}

#[derive(Debug)]
pub enum EqMessage {
    /// The region's capabilities, from its seed (URLs: secrets of the session, never logged).
    Caps(HashMap<String, String>),
    Event(EqEvent),
    /// The queue failed too many times in a row.
    Lost,
}

const ERROR_RETRY: Duration = Duration::from_secs(1);
const ERROR_RETRY_INC: Duration = Duration::from_secs(3);
const MAX_ERRORS: u32 = 15;
/// The region holds a poll at least this long when it has nothing to send.
const MIN_HELD: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollEnd {
    Reply,
    Status(u16),
    Timeout,
    Transport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    Again,
    Wait(Duration),
    Closed,
    GiveUp,
}

/// What follows a poll: a held poll that ended without events is normal, a fast failure backs off
/// like lleventpoll.cpp (1 s + 3 s per error, giving up after 15 in a row).
fn after_poll(end: PollEnd, held: Duration, errors: &mut u32) -> Next {
    let no_event = match end {
        PollEnd::Reply => true,
        PollEnd::Status(404 | 410) => return Next::Closed,
        PollEnd::Status(499 | 500 | 502 | 503 | 504) | PollEnd::Timeout | PollEnd::Transport => {
            held >= MIN_HELD
        }
        PollEnd::Status(_) => false,
    };
    if no_event {
        *errors = 0;
        return Next::Again;
    }
    if *errors >= MAX_ERRORS {
        return Next::GiveUp;
    }
    let wait = ERROR_RETRY + ERROR_RETRY_INC * *errors;
    *errors += 1;
    Next::Wait(wait)
}

/// Long-poll `EventQueueGet` until the receiver is dropped, the region closes the queue or too
/// many polls in a row fail.
pub async fn event_queue_loop(
    http: reqwest::Client,
    url: String,
    tx: mpsc::UnboundedSender<EqMessage>,
) {
    let mut ack = Llsd::Undef;
    let mut errors = 0u32;
    loop {
        if tx.is_closed() {
            return;
        }
        let body = llsd_map! { "ack" => ack.clone(), "done" => false };
        let started = std::time::Instant::now();
        let fut = http
            .post(&url)
            .header("Content-Type", "application/llsd+xml")
            .header("Accept", "application/llsd+xml")
            .timeout(Duration::from_secs(75))
            .body(to_xml(&body))
            .send();
        let (end, why) = match fut.await {
            Ok(resp) => match resp.status().as_u16() {
                200 => match resp.bytes().await {
                    Ok(bytes) => {
                        match from_xml(&bytes) {
                            // a reply without an id keeps the previous ack
                            Ok(v) if !v["id"].is_undef() => {
                                ack = v["id"].clone();
                                for ev in v["events"].as_array() {
                                    let e = EqEvent {
                                        message: ev["message"].as_str().to_owned(),
                                        body: ev["body"].clone(),
                                    };
                                    if tx.send(EqMessage::Event(e)).is_err() {
                                        return;
                                    }
                                }
                            }
                            Ok(_) => log::warn!("event queue: reply without an id"),
                            Err(_) => log::warn!("event queue: malformed LLSD"),
                        }
                        (PollEnd::Reply, String::new())
                    }
                    Err(e) => (PollEnd::Transport, e.without_url().to_string()),
                },
                status => (PollEnd::Status(status), format!("status {status}")),
            },
            Err(e) if e.is_timeout() => (PollEnd::Timeout, String::new()),
            Err(e) => (PollEnd::Transport, e.without_url().to_string()),
        };
        let held = started.elapsed();
        match after_poll(end, held, &mut errors) {
            Next::Again => {}
            Next::Closed => {
                log::info!("event queue closed ({why})");
                return;
            }
            Next::Wait(wait) => {
                log::warn!(
                    "event queue: {why} after {:.1} s, error {errors} of {MAX_ERRORS} in a row, next poll in {} s",
                    held.as_secs_f32(),
                    wait.as_secs()
                );
                tokio::time::sleep(wait).await;
            }
            Next::GiveUp => {
                log::warn!("event queue: {why}, giving up after {MAX_ERRORS} errors in a row");
                let _ = tx.send(EqMessage::Lost);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELD: Duration = Duration::from_secs(30);
    const FAST: Duration = Duration::from_millis(200);

    #[test]
    fn a_quiet_region_never_stops_the_event_queue() {
        let mut errors = 0;
        for _ in 0..100 {
            for end in [
                PollEnd::Transport,
                PollEnd::Status(502),
                PollEnd::Timeout,
                PollEnd::Status(499),
            ] {
                assert_eq!(after_poll(end, HELD, &mut errors), Next::Again);
            }
        }
        assert_eq!(errors, 0);
    }

    #[test]
    fn fast_failures_back_off_then_give_up() {
        let mut errors = 0;
        for n in 0..MAX_ERRORS {
            assert_eq!(
                after_poll(PollEnd::Status(503), FAST, &mut errors),
                Next::Wait(Duration::from_secs(1 + 3 * u64::from(n)))
            );
        }
        assert_eq!(
            after_poll(PollEnd::Transport, FAST, &mut errors),
            Next::GiveUp
        );
    }

    #[test]
    fn a_missing_queue_is_closed_at_once() {
        let mut errors = 0;
        assert_eq!(
            after_poll(PollEnd::Status(404), HELD, &mut errors),
            Next::Closed
        );
    }
}
