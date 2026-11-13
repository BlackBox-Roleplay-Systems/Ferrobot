//! A stand-in Discord on localhost: its gateway (WebSocket: Hello, Ready, heartbeat acks, then
//! the messages a test posts as Discord users) and the REST routes the bot uses, keeping a
//! server's channels, messages, roles and members. The bot reaches it through twilight's proxy
//! settings (FERRO_DISCORD_STAND_IN).

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_websockets::{Message, ServerBuilder};

/// The server, the bot's user, a master and someone else.
pub const SERVER: u64 = 800;
pub const BOT: u64 = 900;
pub const MASTER: u64 = 901;
pub const VISITOR: u64 = 902;
/// A role on the server, held by the master.
pub const MODERATOR: u64 = 700;

#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub id: u64,
    /// 0 text, 4 category, 1 a direct message.
    pub kind: u8,
    pub name: String,
    pub parent: Option<u64>,
    pub topic: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Posted {
    pub id: u64,
    pub channel: u64,
    pub author: u64,
    pub content: String,
    pub tts: bool,
    pub reply_to: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Role {
    pub id: u64,
    pub name: String,
    pub color: u32,
    pub permissions: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub id: u64,
    pub name: String,
    pub nick: Option<String>,
    pub roles: Vec<u64>,
    pub mute: bool,
}

#[derive(Default)]
pub struct State {
    pub channels: Vec<Channel>,
    pub messages: Vec<Posted>,
    pub roles: Vec<Role>,
    pub members: Vec<Member>,
    /// Banned users and the reason given.
    pub bans: Vec<(u64, String)>,
    /// Reactions: message, emoji.
    pub reactions: Vec<(u64, String)>,
    pub deleted_messages: Vec<u64>,
    pub deleted_channels: Vec<u64>,
    /// The bot's activities, as its presence updates set them.
    pub presences: Vec<String>,
    next: u64,
    gateway: Option<mpsc::UnboundedSender<String>>,
    seq: u64,
}

impl State {
    fn id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
}

pub struct StandIn {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
}

fn user(id: u64, name: &str, bot: bool) -> Value {
    json!({"id": id.to_string(), "username": name, "discriminator": "0", "avatar": null, "bot": bot})
}

fn name_of(st: &State, id: u64) -> String {
    match id {
        BOT => "Ferrobot".into(),
        _ => st
            .members
            .iter()
            .find(|m| m.id == id)
            .map_or_else(|| format!("user{id}"), |m| m.name.clone()),
    }
}

fn channel_json(c: &Channel) -> Value {
    let mut v = json!({
        "id": c.id.to_string(), "type": c.kind, "name": c.name, "position": 0,
        "topic": c.topic, "permission_overwrites": [],
    });
    if c.kind != 1 {
        v["guild_id"] = json!(SERVER.to_string());
    }
    if let Some(p) = c.parent {
        v["parent_id"] = json!(p.to_string());
    }
    v
}

fn message_json(st: &State, m: &Posted) -> Value {
    let mut v = json!({
        "id": m.id.to_string(), "channel_id": m.channel.to_string(),
        "author": user(m.author, &name_of(st, m.author), m.author == BOT),
        "content": m.content, "timestamp": "2026-11-13T00:00:00.000000+00:00",
        "edited_timestamp": null, "tts": m.tts, "mention_everyone": false, "mentions": [],
        "mention_roles": [], "mention_channels": [], "attachments": [], "embeds": [],
        "reactions": [], "pinned": false, "type": 0, "components": [], "sticker_items": [],
        "message_snapshots": [],
    });
    if st.channels.iter().any(|c| c.id == m.channel && c.kind != 1) {
        v["guild_id"] = json!(SERVER.to_string());
    }
    v
}

fn role_json(r: &Role) -> Value {
    json!({
        "id": r.id.to_string(), "name": r.name, "color": r.color,
        "colors": {"primary_color": r.color, "secondary_color": null, "tertiary_color": null},
        "hoist": false, "managed": false, "mentionable": false,
        "permissions": r.permissions.to_string(), "position": 1, "flags": 0,
    })
}

fn member_json(m: &Member) -> Value {
    json!({
        "user": user(m.id, &m.name, false), "nick": m.nick,
        "roles": m.roles.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "joined_at": "2026-01-01T00:00:00.000000+00:00", "deaf": false, "mute": m.mute,
        "pending": false, "flags": 0,
    })
}

struct Reply {
    status: u16,
    body: Option<Value>,
}

fn ok(v: Value) -> Reply {
    Reply {
        status: 200,
        body: Some(v),
    }
}

fn empty() -> Reply {
    Reply {
        status: 204,
        body: None,
    }
}

fn not_found() -> Reply {
    Reply {
        status: 404,
        body: Some(json!({"code": 10003, "message": "Unknown"})),
    }
}

impl StandIn {
    pub async fn start() -> std::io::Result<StandIn> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let mut st = State {
            next: 1000,
            ..State::default()
        };
        st.roles.push(Role {
            id: SERVER,
            name: "@everyone".into(),
            color: 0,
            permissions: 1 << 11,
        });
        st.roles.push(Role {
            id: MODERATOR,
            name: "Moderator".into(),
            color: 0x00ff00,
            permissions: (1 << 11) | (1 << 13),
        });
        st.members.push(Member {
            id: MASTER,
            name: "boss".into(),
            nick: Some("The Boss".into()),
            roles: vec![MODERATOR],
            mute: false,
        });
        st.members.push(Member {
            id: VISITOR,
            name: "visitor".into(),
            nick: None,
            roles: vec![],
            mute: false,
        });
        // a channel of the server's own, not the bot's
        let general = st.id();
        st.channels.push(Channel {
            id: general,
            kind: 0,
            name: "general".into(),
            parent: None,
            topic: String::new(),
        });
        let state = Arc::new(Mutex::new(st));
        let s2 = state.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(connection(stream, s2.clone(), addr));
            }
        });
        Ok(StandIn { addr, state })
    }

    /// `host:port`, for FERRO_DISCORD_STAND_IN.
    pub fn host(&self) -> String {
        self.addr.to_string()
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A channel by name (and category name, when given).
    pub fn channel(&self, name: &str, category: Option<&str>) -> Option<Channel> {
        let st = self.state();
        let parent = category.and_then(|c| {
            st.channels
                .iter()
                .find(|x| x.kind == 4 && x.name == c)
                .map(|x| x.id)
        });
        st.channels
            .iter()
            .find(|c| c.kind == 0 && c.name == name && (category.is_none() || c.parent == parent))
            .cloned()
    }

    /// What the bot posted in a channel.
    pub fn posts(&self, channel: u64) -> Vec<Posted> {
        self.state()
            .messages
            .iter()
            .filter(|m| m.channel == channel && m.author == BOT)
            .cloned()
            .collect()
    }

    /// A Discord user writes in a channel: the message is kept, and the gateway tells the bot.
    pub fn say(&self, channel: u64, author: u64, content: &str) -> u64 {
        let mut st = self.state();
        let id = st.id();
        let m = Posted {
            id,
            channel,
            author,
            content: content.into(),
            tts: false,
            reply_to: None,
        };
        st.messages.push(m.clone());
        let payload = message_json(&st, &m);
        st.seq += 1;
        let frame = json!({"op": 0, "t": "MESSAGE_CREATE", "s": st.seq, "d": payload}).to_string();
        if let Some(tx) = &st.gateway {
            let _ = tx.send(frame);
        }
        id
    }

    /// Until `done` holds, or the time is out. `done` gets the stand-in, not its locked state:
    /// it may ask anything of it.
    pub async fn wait(&self, timeout: Duration, done: impl Fn(&StandIn) -> bool) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if done(self) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

async fn connection(stream: TcpStream, state: Arc<Mutex<State>>, addr: SocketAddr) {
    let mut head = [0u8; 2048];
    let Ok(n) = stream.peek(&mut head).await else {
        return;
    };
    let text = String::from_utf8_lossy(&head[..n]).to_lowercase();
    if text.starts_with("get /gateway") && text.contains("upgrade: websocket") {
        gateway(stream, state, addr).await;
    } else {
        rest(stream, state).await;
    }
}

/// The gateway: Hello, then Ready on Identify, acks for the heartbeats, the presence kept, and
/// the events a test sends.
async fn gateway(stream: TcpStream, state: Arc<Mutex<State>>, addr: SocketAddr) {
    let Ok((_, mut ws)) = ServerBuilder::new().accept(stream).await else {
        return;
    };
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let hello = json!({"op": 10, "d": {"heartbeat_interval": 41250}}).to_string();
    if ws.send(Message::text(hello)).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            frame = ws.next() => {
                let Some(Ok(frame)) = frame else { return };
                let Some(text) = frame.as_text() else { continue };
                let Ok(v) = serde_json::from_str::<Value>(text) else { continue };
                match v["op"].as_u64() {
                    Some(1) => {
                        let ack = json!({"op": 11}).to_string();
                        let _ = ws.send(Message::text(ack)).await;
                    }
                    Some(2) | Some(6) => {
                        let ready = {
                            let mut st = state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                            st.gateway = Some(tx.clone());
                            st.seq += 1;
                            json!({"op": 0, "t": "READY", "s": st.seq, "d": {
                                "v": 10,
                                "user": {"id": BOT.to_string(), "username": "Ferrobot", "discriminator": "0",
                                         "avatar": null, "bot": true, "mfa_enabled": false},
                                "guilds": [{"id": SERVER.to_string(), "unavailable": true}],
                                "session_id": "stand-in",
                                "resume_gateway_url": format!("ws://{addr}/gateway"),
                                "shard": [0, 1],
                                "application": {"id": BOT.to_string(), "flags": 0},
                            }}).to_string()
                        };
                        let _ = ws.send(Message::text(ready)).await;
                    }
                    Some(3) => {
                        let name = v["d"]["activities"][0]["name"].as_str().unwrap_or_default().to_owned();
                        state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).presences.push(name);
                    }
                    _ => {}
                }
            }
            out = rx.recv() => {
                let Some(out) = out else { return };
                if ws.send(Message::text(out)).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// One REST request, answered, the connection closed.
async fn rest(mut stream: TcpStream, state: Arc<Mutex<State>>) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let (head_end, length) = loop {
        let Ok(n) = stream.read(&mut chunk).await else {
            return;
        };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..i]).to_lowercase();
            let length = head
                .lines()
                .find_map(|l| {
                    l.strip_prefix("content-length:")
                        .map(|v| v.trim().parse().unwrap_or(0))
                })
                .unwrap_or(0usize);
            break (i + 4, length);
        }
    };
    while buf.len() < head_end + length {
        let Ok(n) = stream.read(&mut chunk).await else {
            return;
        };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut first = head.lines().next().unwrap_or_default().split(' ');
    let method = first.next().unwrap_or_default().to_owned();
    let target = first.next().unwrap_or_default().to_owned();
    let body: Value = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
    let path = target.split('?').next().unwrap_or_default();
    let path = path.strip_prefix("/api/v10").unwrap_or(path);
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    let reply = {
        let mut st = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        route(&mut st, &method, &parts, &body)
    };
    let (text, body) = match reply.body {
        Some(v) => (
            if reply.status < 300 {
                "OK"
            } else {
                "Not Found"
            },
            v.to_string(),
        ),
        None => ("No Content", String::new()),
    };
    let response = format!(
        "HTTP/1.1 {} {text}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        reply.status,
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn num(s: &str) -> u64 {
    s.parse().unwrap_or(0)
}

fn route(st: &mut State, method: &str, p: &[&str], body: &Value) -> Reply {
    let guild = SERVER.to_string();
    match (method, p) {
        ("GET", ["guilds", g, "channels"]) if *g == guild => ok(Value::Array(
            st.channels
                .iter()
                .filter(|c| c.kind != 1)
                .map(channel_json)
                .collect(),
        )),
        ("POST", ["guilds", g, "channels"]) if *g == guild => {
            let id = st.id();
            let c = Channel {
                id,
                kind: body["type"].as_u64().unwrap_or(0) as u8,
                name: body["name"].as_str().unwrap_or_default().into(),
                parent: body["parent_id"].as_str().map(num),
                topic: body["topic"].as_str().unwrap_or_default().into(),
            };
            st.channels.push(c.clone());
            ok(channel_json(&c))
        }
        ("GET", ["channels", c]) => match st.channels.iter().find(|x| x.id == num(c)) {
            Some(x) => ok(channel_json(x)),
            None => not_found(),
        },
        ("DELETE", ["channels", c]) => {
            let id = num(c);
            st.channels.retain(|x| x.id != id);
            st.deleted_channels.push(id);
            ok(json!({"id": c, "type": 0}))
        }
        ("POST", ["channels", c, "messages"]) => {
            let channel = num(c);
            if !st.channels.iter().any(|x| x.id == channel) {
                return not_found();
            }
            let id = st.id();
            let m = Posted {
                id,
                channel,
                author: BOT,
                content: body["content"].as_str().unwrap_or_default().into(),
                tts: body["tts"].as_bool().unwrap_or(false),
                reply_to: body["message_reference"]["message_id"].as_str().map(num),
            };
            st.messages.push(m.clone());
            ok(message_json(st, &m))
        }
        ("GET", ["channels", c, "messages"]) => {
            let channel = num(c);
            let list: Vec<Value> = st
                .messages
                .iter()
                .filter(|m| m.channel == channel)
                .rev()
                .take(100)
                .map(|m| message_json(st, m))
                .collect();
            ok(Value::Array(list))
        }
        ("DELETE", ["channels", _, "messages", m]) => {
            let id = num(m);
            st.messages.retain(|x| x.id != id);
            st.deleted_messages.push(id);
            empty()
        }
        ("POST", ["channels", _, "messages", "bulk-delete"]) => {
            let ids: Vec<u64> = body["messages"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(num)).collect())
                .unwrap_or_default();
            st.messages.retain(|x| !ids.contains(&x.id));
            st.deleted_messages.extend(ids);
            empty()
        }
        ("PUT", ["channels", _, "messages", m, "reactions", emoji, "@me"]) => {
            let emoji = percent_decode(emoji);
            st.reactions.push((num(m), emoji));
            empty()
        }
        ("GET", ["guilds", g, "roles"]) if *g == guild => {
            ok(Value::Array(st.roles.iter().map(role_json).collect()))
        }
        ("GET", ["guilds", g, "roles", "member-counts"]) if *g == guild => {
            let counts: BTreeMap<String, usize> = st
                .roles
                .iter()
                .map(|r| {
                    let n = st
                        .members
                        .iter()
                        .filter(|m| m.roles.contains(&r.id))
                        .count();
                    (r.id.to_string(), n)
                })
                .collect();
            ok(json!(counts))
        }
        ("POST", ["guilds", g, "roles"]) if *g == guild => {
            let id = st.id();
            let r = Role {
                id,
                name: body["name"].as_str().unwrap_or_default().into(),
                color: body["color"].as_u64().unwrap_or(0) as u32,
                permissions: body["permissions"].as_str().map_or(0, num),
            };
            st.roles.push(r.clone());
            ok(role_json(&r))
        }
        ("PATCH", ["guilds", g, "roles", r]) if *g == guild => {
            let id = num(r);
            let Some(role) = st.roles.iter_mut().find(|x| x.id == id) else {
                return not_found();
            };
            if let Some(p) = body["permissions"].as_str() {
                role.permissions = num(p);
            }
            let v = role_json(role);
            ok(v)
        }
        ("DELETE", ["guilds", g, "roles", r]) if *g == guild => {
            let id = num(r);
            let before = st.roles.len();
            st.roles.retain(|x| x.id != id);
            if st.roles.len() == before {
                not_found()
            } else {
                empty()
            }
        }
        ("GET", ["guilds", g, "members"]) if *g == guild => {
            ok(Value::Array(st.members.iter().map(member_json).collect()))
        }
        ("GET", ["guilds", g, "members", u]) if *g == guild => {
            match st.members.iter().find(|m| m.id == num(u)) {
                Some(m) => ok(member_json(m)),
                None => not_found(),
            }
        }
        ("PATCH", ["guilds", g, "members", u]) if *g == guild => {
            let Some(m) = st.members.iter_mut().find(|m| m.id == num(u)) else {
                return not_found();
            };
            if let Some(mute) = body["mute"].as_bool() {
                m.mute = mute;
            }
            let v = member_json(m);
            ok(v)
        }
        ("PUT", ["guilds", g, "members", u, "roles", r]) if *g == guild => {
            let Some(m) = st.members.iter_mut().find(|m| m.id == num(u)) else {
                return not_found();
            };
            m.roles.push(num(r));
            empty()
        }
        ("PUT", ["guilds", g, "bans", u]) if *g == guild => {
            st.bans.push((num(u), String::new()));
            empty()
        }
        ("POST", ["users", "@me", "channels"]) => {
            let recipient = body["recipient_id"].as_str().map_or(0, num);
            let id = st.id();
            let c = Channel {
                id,
                kind: 1,
                name: format!("dm-{recipient}"),
                parent: None,
                topic: String::new(),
            };
            st.channels.push(c.clone());
            ok(channel_json(&c))
        }
        _ => not_found(),
    }
}

/// A reaction's emoji, percent-encoded in the path.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
