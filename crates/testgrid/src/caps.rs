//! The region's capabilities: the seed, and `EventQueueGet`, held like the grid's long poll.

use crate::http::Response;
use crate::{Seen, Shared};
use ferro_llsd::{Llsd, Map, from_xml, to_xml};
use std::sync::Arc;
use tokio::time::Instant;

pub async fn handle(shared: &Arc<Shared>, n: u32, name: &str, body: &[u8]) -> Response {
    match shared.state().sessions.get(&n) {
        Some(s) if !s.ended => {}
        // the capabilities of a session that ended are gone
        _ => return Response::status(404),
    }
    match name {
        "seed" => seed(shared, n, body),
        "eq" => event_queue(shared, n).await,
        _ => Response::status(404),
    }
}

/// The seed answers with the capabilities it has among those asked for.
fn seed(shared: &Arc<Shared>, n: u32, body: &[u8]) -> Response {
    let Ok(asked) = from_xml(body) else {
        return Response::status(400);
    };
    let caps: Vec<String> = asked
        .as_array()
        .iter()
        .map(|c| c.as_str().to_owned())
        .collect();
    shared.saw(Seen::SeedRequest {
        session: n,
        caps: caps.clone(),
    });
    let mut m = Map::new();
    if caps.iter().any(|c| c == "EventQueueGet") {
        m.insert(
            "EventQueueGet".into(),
            format!("http://{}/cap/{n}/eq", shared.http_addr).into(),
        );
    }
    llsd_reply(&Llsd::Map(m))
}

/// A poll is answered at once when events wait, else held until one comes or `eq_hold` passes,
/// then 502 as the grid's queue answers an idle poll. A closed queue or an ended session: 404.
async fn event_queue(shared: &Arc<Shared>, n: u32) -> Response {
    shared.saw(Seen::EventQueuePoll { session: n });
    let deadline = Instant::now() + shared.cfg.eq_hold;
    loop {
        let wake = shared.eq_wake.notified();
        tokio::pin!(wake);
        wake.as_mut().enable();
        {
            let mut st = shared.state();
            let Some(s) = st.sessions.get_mut(&n) else {
                return Response::status(404);
            };
            if s.ended || s.close_queue {
                return Response::status(404);
            }
            if !s.events.is_empty() {
                s.eq_id += 1;
                let events: Vec<Llsd> = s
                    .events
                    .drain(..)
                    .map(|(message, body)| {
                        let mut e = Map::new();
                        e.insert("message".into(), message.into());
                        e.insert("body".into(), body);
                        Llsd::Map(e)
                    })
                    .collect();
                let mut m = Map::new();
                m.insert("events".into(), Llsd::Array(events));
                m.insert("id".into(), s.eq_id.into());
                return llsd_reply(&Llsd::Map(m));
            }
        }
        if tokio::time::timeout_at(deadline, wake).await.is_err() {
            return Response::status(502);
        }
    }
}

fn llsd_reply(v: &Llsd) -> Response {
    Response::new(200, "application/llsd+xml", to_xml(v))
}
