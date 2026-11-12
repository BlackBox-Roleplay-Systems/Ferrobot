//! The login server: `login_to_simulator`, answered as the grid answers it (the fields the
//! viewer's LLLoginInstance and the bot read), or refused as the grid refuses a wrong password.

use crate::http::Response;
use crate::{Seen, Session, Shared, xmlrpc};
use ferro_llsd::{Llsd, Map};
use std::collections::VecDeque;
use std::sync::Arc;
use uuid::Uuid;

pub fn handle(shared: &Arc<Shared>, body: &[u8]) -> Response {
    let Some((method, p)) = xmlrpc::parse_call(body) else {
        return Response::status(400);
    };
    if method != "login_to_simulator" {
        return refuse(shared, "key", "unknown login method");
    }
    let cfg = &shared.cfg;
    let password = format!("$1${:x}", md5::compute(cfg.password.as_bytes()));
    if !p["first"].as_str().eq_ignore_ascii_case(&cfg.first)
        || !p["last"].as_str().eq_ignore_ascii_case(&cfg.last)
        || p["passwd"].as_str() != password
    {
        return refuse(
            shared,
            "key",
            "Sorry! We couldn't log you in. Please check that your username and password are correct.",
        );
    }

    let mut st = shared.state();
    // a new login ends the account's previous session, as the grid does
    for s in st.sessions.values_mut() {
        s.ended = true;
    }
    let n = st.sessions.len() as u32 + 1;
    let session = Session {
        n,
        session_id: Uuid::new_v4(),
        circuit_code: 1000 + n,
        ended: false,
        region: crate::FIRST_REGION,
        expected: None,
        arrival: crate::region::ARRIVAL,
        events: VecDeque::new(),
        close_queue: false,
        eq_id: 0,
    };
    let mut r = Map::new();
    r.insert("login".into(), "true".into());
    r.insert("agent_id".into(), shared.agent_id.into());
    r.insert("session_id".into(), session.session_id.into());
    r.insert("secure_session_id".into(), Uuid::new_v4().into());
    // the grid quotes the first name
    r.insert("first_name".into(), format!("\"{}\"", cfg.first).into());
    r.insert("last_name".into(), cfg.last.clone().into());
    r.insert("circuit_code".into(), (session.circuit_code as i32).into());
    let region = &shared.regions[crate::FIRST_REGION];
    r.insert("sim_ip".into(), region.addr.ip().to_string().into());
    r.insert("sim_port".into(), i32::from(region.addr.port()).into());
    r.insert("region_x".into(), (region.x as i32).into());
    r.insert("region_y".into(), (region.y as i32).into());
    r.insert(
        "seed_capability".into(),
        shared.seed_url(n, crate::FIRST_REGION).into(),
    );
    r.insert("look_at".into(), "[r1,r0,r0]".into());
    r.insert("start_location".into(), p["start"].as_str().into());
    r.insert("agent_access".into(), "M".into());
    if !cfg.home.is_empty() {
        r.insert("home".into(), cfg.home.clone().into());
    }
    r.insert("message".into(), "Welcome to the test grid".into());
    let mut root = Map::new();
    root.insert("folder_id".into(), Uuid::new_v4().into());
    r.insert("inventory-root".into(), Llsd::Array(vec![Llsd::Map(root)]));
    st.sessions.insert(n, session);
    drop(st);
    shared.eq_wake.notify_waiters();

    shared.saw(Seen::Login {
        session: n,
        first: p["first"].as_str().into(),
        last: p["last"].as_str().into(),
        start: p["start"].as_str().into(),
        channel: p["channel"].as_str().into(),
    });
    Response::new(200, "text/xml", xmlrpc::response(&Llsd::Map(r)))
}

fn refuse(shared: &Arc<Shared>, reason: &str, message: &str) -> Response {
    shared.saw(Seen::LoginRefused {
        reason: reason.into(),
    });
    let mut r = Map::new();
    r.insert("login".into(), "false".into());
    r.insert("reason".into(), reason.into());
    r.insert("message".into(), message.into());
    Response::new(200, "text/xml", xmlrpc::response(&Llsd::Map(r)))
}
