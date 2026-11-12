//! The regions' capabilities: the seed, and `EventQueueGet`, held like the grid's long poll.
//! A session's capabilities answer in the region its agent is in, and in the one a teleport is
//! taking it to; elsewhere, and once the session ended, they are gone (404).

use crate::http::Response;
use crate::{Seen, Shared};
use ferro_llsd::{Llsd, Map, from_xml, to_xml};
use std::sync::Arc;
use tokio::time::Instant;

pub async fn handle(
    shared: &Arc<Shared>,
    n: u32,
    region: usize,
    name: &str,
    method: &str,
    query: &str,
    body: &[u8],
) -> Response {
    match shared.state().sessions.get(&n) {
        Some(s) if !s.ended && (s.region == region || s.expected == Some(region)) => {}
        _ => return Response::status(404),
    }
    match (name, method) {
        ("seed", "POST") => seed(shared, n, region, body),
        ("eq", "POST") => event_queue(shared, n, region).await,
        ("displaynames", "GET") => display_names(shared, n, query),
        _ => Response::status(404),
    }
}

/// The seed answers with the capabilities it has among those asked for.
fn seed(shared: &Arc<Shared>, n: u32, region: usize, body: &[u8]) -> Response {
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
        region,
        caps: caps.clone(),
    });
    let mut m = Map::new();
    if shared.cfg.display_names_cap && caps.iter().any(|c| c == "GetDisplayNames") {
        // granted without a trailing slash, as the grid does
        m.insert(
            "GetDisplayNames".into(),
            format!("http://{}/cap/{n}/{region}/displaynames", shared.http_addr).into(),
        );
    }
    if caps.iter().any(|c| c == "EventQueueGet") {
        m.insert(
            "EventQueueGet".into(),
            format!("http://{}/cap/{n}/{region}/eq", shared.http_addr).into(),
        );
    }
    llsd_reply(&Llsd::Map(m))
}

/// A poll is answered at once when events wait, else held until one comes or `eq_hold` passes,
/// then 502 as the grid's queue answers an idle poll. Events go out on the queue of the region
/// the agent is in. A closed queue, an ended session, a region the agent left: 404.
async fn event_queue(shared: &Arc<Shared>, n: u32, region: usize) -> Response {
    shared.saw(Seen::EventQueuePoll { session: n, region });
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
            let here = s.region == region;
            if s.ended || (here && s.close_queue) || (!here && s.expected != Some(region)) {
                return Response::status(404);
            }
            if here && !s.events.is_empty() {
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

/// The People API: `?ids=<id>&ids=<id>…`; `agents` for the avatars the grid knows, `bad_ids`
/// for the others (LLAvatarNameCache reads both).
fn display_names(shared: &Arc<Shared>, n: u32, query: &str) -> Response {
    let ids: Vec<uuid::Uuid> = query
        .split('&')
        .filter_map(|p| p.strip_prefix("ids="))
        .filter_map(|id| id.parse().ok())
        .collect();
    shared.saw(Seen::DisplayNamesRequest {
        session: n,
        ids: ids.clone(),
    });
    let cfg = &shared.cfg;
    let mut agents = Vec::new();
    let mut bad = Vec::new();
    for id in ids {
        let Some(r) = cfg.residents.iter().find(|r| r.id == id) else {
            bad.push(Llsd::from(id));
            continue;
        };
        let username = if r.last == "Resident" {
            r.first.to_lowercase()
        } else {
            format!("{}.{}", r.first, r.last).to_lowercase()
        };
        let chosen = cfg
            .display_names
            .iter()
            .find(|(who, _)| *who == id)
            .map(|(_, name)| name.clone());
        let mut a = Map::new();
        a.insert("id".into(), id.into());
        a.insert("username".into(), username.into());
        a.insert(
            "display_name".into(),
            chosen
                .clone()
                .unwrap_or_else(|| format!("{} {}", r.first, r.last))
                .into(),
        );
        a.insert("legacy_first_name".into(), r.first.clone().into());
        a.insert("legacy_last_name".into(), r.last.clone().into());
        a.insert("is_display_name_default".into(), chosen.is_none().into());
        agents.push(Llsd::Map(a));
    }
    let mut m = Map::new();
    m.insert("agents".into(), Llsd::Array(agents));
    m.insert("bad_ids".into(), Llsd::Array(bad));
    llsd_reply(&Llsd::Map(m))
}

fn llsd_reply(v: &Llsd) -> Response {
    Response::new(200, "application/llsd+xml", to_xml(v))
}
