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
        ("groupmemberdata", "POST") => member_data(shared, body),
        ("chatsessionrequest", "POST") => chat_session_request(shared, body),
        ("groupapi", "GET" | "POST") => group_api(shared, method, query, body),
        ("appearance", "POST") => appearance(shared, body),
        ("viewerasset", "GET") => {
            let want = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("gesture_id="))
                .and_then(|v| uuid::Uuid::parse_str(v).ok());
            match want {
                Some(id) if id == crate::inventory::GESTURE_ASSET => {
                    shared.saw(Seen::ViewerAsset { asset: id });
                    Response::new(200, "application/octet-stream", crate::inventory::gesture())
                }
                _ => Response::status(404),
            }
        }
        ("estateaccess", "GET") => {
            // the bot manages the estate; one resident is banned
            let entry = |k: &str, id: uuid::Uuid| {
                let mut m = Map::new();
                m.insert(k.into(), id.into());
                Llsd::Map(m)
            };
            let mut m = Map::new();
            m.insert(
                "Managers".into(),
                Llsd::Array(vec![entry("agent_id", shared.agent_id)]),
            );
            m.insert(
                "BannedAgents".into(),
                Llsd::Array(vec![entry("id", uuid::Uuid::from_u128(0xba11_0001))]),
            );
            m.insert("AllowedAgents".into(), Llsd::Array(vec![]));
            m.insert("AllowedGroups".into(), Llsd::Array(vec![]));
            llsd_reply(&Llsd::Map(m))
        }
        ("regioninfo", "POST") | ("estatechangeinfo", "POST") => {
            let Ok(v) = from_xml(body) else {
                return Response::status(400);
            };
            shared.saw(Seen::EstatePost {
                cap: name.to_owned(),
                body: ferro_llsd::to_xml_string(&v),
            });
            llsd_reply(&Llsd::Map(Map::new()))
        }
        ("remoteparcel", "POST") => {
            let mut m = Map::new();
            m.insert("parcel_id".into(), crate::parcels::PARCEL_ID.into());
            llsd_reply(&Llsd::Map(m))
        }
        ("parcelupdate", "POST") => {
            let Ok(v) = from_xml(body) else {
                return Response::status(400);
            };
            let parcel = {
                let mut st = shared.state();
                st.parcel.apply(&v);
                st.parcel.clone()
            };
            shared.saw(Seen::ParcelUpdate {
                parcel: Box::new(parcel.clone()),
            });
            // the region answers with the parcel as it is now
            shared.queue_event(n, "ParcelProperties", parcel.properties(0, shared.agent_id));
            llsd_reply(&Llsd::Map(Map::new()))
        }
        ("preferences", "POST") => {
            let Ok(v) = from_xml(body) else {
                return Response::status(400);
            };
            shared.saw(Seen::HoverHeight {
                height: format!("{:.1}", v["hover_height"].as_f64()),
            });
            llsd_reply(&Llsd::Map(Map::new()))
        }
        ("notecardsave", "POST") => {
            let Ok(v) = from_xml(body) else {
                return Response::status(400);
            };
            let mut m = Map::new();
            m.insert("state".into(), "upload".into());
            m.insert(
                "uploader".into(),
                format!(
                    "http://{}/cap/{n}/{region}/uploader?item={}",
                    shared.http_addr,
                    v["item_id"].as_uuid()
                )
                .into(),
            );
            llsd_reply(&Llsd::Map(m))
        }
        ("uploader", "POST") => {
            let item: uuid::Uuid = query
                .strip_prefix("item=")
                .and_then(|i| i.parse().ok())
                .unwrap_or_default();
            let asset = uuid::Uuid::new_v4();
            if let Some(i) = shared
                .state()
                .inventory
                .items
                .iter_mut()
                .find(|i| i.id == item)
            {
                i.asset_id = asset;
            }
            shared.saw(Seen::NotecardSaved {
                item,
                asset: String::from_utf8_lossy(body).into_owned(),
            });
            let mut m = Map::new();
            m.insert("state".into(), "complete".into());
            m.insert("new_asset".into(), asset.into());
            llsd_reply(&Llsd::Map(m))
        }
        ("fetchdescendents", "POST") => {
            let Ok(v) = from_xml(body) else {
                return Response::status(400);
            };
            let folders: Vec<uuid::Uuid> = v["folders"]
                .as_array()
                .iter()
                .map(|f| f["folder_id"].as_uuid())
                .collect();
            let reply = shared
                .state()
                .inventory
                .descendents(&folders, shared.agent_id);
            llsd_reply(&reply)
        }
        ("fetchitems", "POST") => {
            let Ok(v) = from_xml(body) else {
                return Response::status(400);
            };
            let ids: Vec<uuid::Uuid> = v["items"]
                .as_array()
                .iter()
                .map(|i| i["item_id"].as_uuid())
                .collect();
            let reply = shared.state().inventory.items(&ids, shared.agent_id);
            llsd_reply(&reply)
        }
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
    for (cap, path) in [
        ("FetchInventoryDescendents2", "fetchdescendents"),
        ("FetchInventory2", "fetchitems"),
        ("UpdateNotecardAgentInventory", "notecardsave"),
        ("AgentPreferences", "preferences"),
        ("RemoteParcelRequest", "remoteparcel"),
        ("EstateAccess", "estateaccess"),
        ("ViewerAsset", "viewerasset"),
        ("DispatchRegionInfo", "regioninfo"),
        ("EstateChangeInfo", "estatechangeinfo"),
        ("ParcelPropertiesUpdate", "parcelupdate"),
    ] {
        if caps.iter().any(|c| c == cap) {
            m.insert(
                cap.into(),
                format!("http://{}/cap/{n}/{region}/{path}", shared.http_addr).into(),
            );
        }
    }
    if caps.iter().any(|c| c == "UpdateAvatarAppearance") {
        m.insert(
            "UpdateAvatarAppearance".into(),
            format!("http://{}/cap/{n}/{region}/appearance", shared.http_addr).into(),
        );
    }
    if caps.iter().any(|c| c == "GroupAPIv1") {
        m.insert(
            "GroupAPIv1".into(),
            format!("http://{}/cap/{n}/{region}/groupapi", shared.http_addr).into(),
        );
    }
    if caps.iter().any(|c| c == "ChatSessionRequest") {
        m.insert(
            "ChatSessionRequest".into(),
            format!(
                "http://{}/cap/{n}/{region}/chatsessionrequest",
                shared.http_addr
            )
            .into(),
        );
    }
    if shared.cfg.member_data_cap && caps.iter().any(|c| c == "GroupMemberData") {
        m.insert(
            "GroupMemberData".into(),
            format!(
                "http://{}/cap/{n}/{region}/groupmemberdata",
                shared.http_addr
            )
            .into(),
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

/// GroupMemberData: `{group_id}` in, the whole member list out; a group the bot is not in: 403.
fn member_data(shared: &Arc<Shared>, body: &[u8]) -> Response {
    let Ok(asked) = from_xml(body) else {
        return Response::status(400);
    };
    let group = asked["group_id"].as_uuid();
    shared.saw(Seen::GroupMembersRequest { group, cap: true });
    let found = shared
        .state()
        .groups
        .iter()
        .find(|g| g.id == group && g.bot_member)
        .cloned();
    match found {
        Some(g) => llsd_reply(&crate::groups::member_data(&g, shared.agent_id)),
        None => Response::status(403),
    }
}

/// GroupAPIv1, the group bans: `?group_id=`; GET the list, POST `{ban_action, ban_ids}`. A group
/// the bot is not in, or where it has no ban power: 403.
fn group_api(shared: &Arc<Shared>, method: &str, query: &str, body: &[u8]) -> Response {
    let group = query
        .split('&')
        .find_map(|p| p.strip_prefix("group_id="))
        .and_then(|g| g.parse::<uuid::Uuid>().ok())
        .unwrap_or_default();
    let mut st = shared.state();
    let allowed = st
        .groups
        .iter()
        .any(|g| g.id == group && g.bot_member && g.powers & crate::groups::GROUP_BAN_ACCESS != 0);
    if !allowed {
        return Response::status(403);
    }
    if method == "POST" {
        let Ok(v) = from_xml(body) else {
            return Response::status(400);
        };
        let ids: Vec<uuid::Uuid> = v["ban_ids"].as_array().iter().map(Llsd::as_uuid).collect();
        let ban = v["ban_action"].as_i32() == 1;
        let list = st.bans.entry(group).or_default();
        for id in &ids {
            list.retain(|b| b != id);
            if ban {
                list.push(*id);
            }
        }
        drop(st);
        shared.saw(Seen::GroupBanChange { group, ids, ban });
        return llsd_reply(&Llsd::Map(Map::new()));
    }
    let mut list = Map::new();
    for id in st.bans.get(&group).into_iter().flatten() {
        list.insert(id.to_string(), Llsd::Map(Map::new()));
    }
    let mut v = Map::new();
    v.insert("group_id".into(), group.into());
    v.insert("ban_list".into(), Llsd::Map(list));
    llsd_reply(&Llsd::Map(v))
}

/// The outfit's version, as the grid keeps it.
const COF_VERSION: i32 = 7;

/// UpdateAvatarAppearance: the outfit baked when the version is right, else the one expected, as
/// the grid answers (LLAppearanceMgr reads `success`, `error`, `expected`).
fn appearance(shared: &Arc<Shared>, body: &[u8]) -> Response {
    let Ok(v) = from_xml(body) else {
        return Response::status(400);
    };
    let version = v["cof_version"].as_i32();
    shared.saw(Seen::AppearanceRequest {
        cof_version: version,
    });
    let mut m = Map::new();
    if version == COF_VERSION {
        m.insert("success".into(), true.into());
    } else {
        m.insert("success".into(), false.into());
        m.insert("error".into(), "Cof Version Mismatch".into());
        m.insert("expected".into(), COF_VERSION.into());
    }
    llsd_reply(&Llsd::Map(m))
}

/// ChatSessionRequest: "accept invitation" joins the bot to a group chat.
fn chat_session_request(shared: &Arc<Shared>, body: &[u8]) -> Response {
    let Ok(v) = from_xml(body) else {
        return Response::status(400);
    };
    let group = v["session-id"].as_uuid();
    if v["method"].as_str() == "accept invitation" {
        shared.saw(Seen::GroupChatAccept { group });
        shared.state().group_chats.insert(group);
    }
    llsd_reply(&Llsd::Map(Map::new()))
}

fn llsd_reply(v: &Llsd) -> Response {
    Response::new(200, "application/llsd+xml", to_xml(v))
}
