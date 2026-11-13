//! Replies written by an OpenAI-compatible chat completions API, as SecondBot's ChatGptService
//! (Services/ChatGptService.cs, Config/ChatGptConfig.cs). Text only: SecondBot's voice paths
//! (what is heard on voice, replies spoken) are left out, the container has no audio library.
//!
//! What it answers, each switched on by its own setting:
//! - local chat said, whispered or shouted by an avatar (not objects, not the system, not the
//!   bot, not "Second Life"), in one conversation for everyone, with one cooldown;
//! - IMs from avatars (friends only if asked), one conversation and one cooldown per avatar;
//! - the chat of one group, one conversation and one cooldown for the group.
//!
//! Each conversation starts with SecondBot's system line ("You are <name>, <prompt>, you are
//! talking to …"), keeps its last 3 to 10 lines, and is forgotten after a quiet spell. Each line
//! asked is "<name> says <message>". The answer is only ever said back, in local chat, by IM or
//! in the group's chat: it never runs a command.
//!
//! Deliberate differences with SecondBot, listed in TASKS.md: no voice, no Redis store of the
//! conversations; the group reply is turned on by AllowGroupReplys (SecondBot's check is
//! inverted) and goes to the group's chat (SecondBot's goes to the sender's key, which is no
//! group); a group is one conversation with one cooldown (SecondBot keeps one per member, mixed
//! with that member's IMs); the history size and cooldowns are clamped as SecondBot meant to
//! (its InRange takes its arguments in the wrong order, which makes them always 10); quiet
//! conversations are forgotten as they age (SecondBot forgets them only when its status is
//! asked); an IM that is a command for the bot is not sent to the API (it may carry a
//! signature); at most 100 conversations and 4 requests at once, the reply cut to 2046 bytes
//! (two chat lines), the answer read up to 1 MiB, 60 s for the whole request, no redirects.

use crate::api::signing::unpack;
use ferro_client::events::{chat_type, dialog};
use ferro_client::{Client, Event};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

/// SecondBot's defaults (Config/ChatGptConfig.cs).
pub const DEFAULT_MODEL: &str = "gpt-3.5-turbo";
pub const DEFAULT_PROMPT: &str = "respond as if you are a horse that knows its going to the glue factory and you are upset about this fact";
pub const OPENAI: &str = "openai";
const OPENAI_BASE: &str = "https://api.openai.com";

/// ANIM_AGENT_TYPE (llanimationstates.cpp): the typing animation, played while the reply is
/// "typed" in local chat (SecondBot's FakeTypeDelay).
const TYPE_ANIMATION: Uuid = Uuid::from_u128(0xc541c47f_e0c0_058b_ad1a_d6ae3a4584d9);
/// The names SecondBot never answers in local chat (hard_blocked_agents).
const BLOCKED: [&str; 2] = ["secondlife", "second life"];
/// EChatType's CHAT_TYPE_REGION_SAYTO (llchat.h).
const REGION_SAY_TO: u8 = 9;
/// The whole request, the answer included. SecondBot's HttpClient waits 100 s.
const TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// The most of an answer read: a chat completion is a few kilobytes.
const MAX_BODY: usize = 1024 * 1024;
/// The most of a reply said: two chat lines (MAX_MSG_BUF_SIZE, 1023 bytes each).
pub const MAX_REPLY: usize = 2 * 1023;
/// The longest cooldown, in seconds (SecondBot's InRange(1, 10)).
pub const MAX_COOLDOWN: u64 = 10;
/// Conversations kept at once: the oldest is forgotten first.
const MAX_WINDOWS: usize = 100;
/// Requests to the API at once; more are not asked, as a cooldown would refuse them.
const MAX_IN_FLIGHT: usize = 4;
/// How much of an error the API sends is logged.
const MAX_ERROR: usize = 300;

#[derive(Clone)]
pub struct Settings {
    /// The API's key (SecondBot's ApiKey). A secret: never logged, never sent but to the API.
    pub api_key: Option<String>,
    /// OpenAI's organization (OrganizationId), sent as OpenAI-Organization.
    pub organization: Option<String>,
    /// "openai", or another provider's address (SecondBot's Provider, the library's BaseDomain).
    pub provider: String,
    /// Where the requests go, from the provider.
    pub endpoint: String,
    /// The model, as the provider names it (UseModel).
    pub model: String,
    /// What the bot is told to be (ChatPrompt).
    pub prompt: String,
    /// The name the bot is told it has (CustomName); None: its first name.
    pub name: Option<String>,
    /// Lines kept after the system line, 3 to 10 (ChatHistoryMessages).
    pub history: usize,
    /// A conversation quiet this long is forgotten (ChatHistoryTimeout, minutes).
    pub history_timeout: Duration,
    /// Local chat (LocalchatReply), and its cooldown in seconds, 1 to 10 (LocalchatRateLimiter).
    pub localchat: bool,
    pub localchat_cooldown: u64,
    /// IMs (AllowImReplys), from friends only (ImReplyFriendsOnly), the cooldown per avatar
    /// (ImReplyRateLimiter).
    pub im: bool,
    pub im_friends_only: bool,
    pub im_cooldown: u64,
    /// The group whose chat is answered (AllowGroupReplys and GroupReplyForGroup), its cooldown
    /// (GroupReplyRateLimiter).
    pub group: Option<Uuid>,
    pub group_cooldown: u64,
    /// What is asked and answered, logged at info rather than debug (ShowDebug).
    pub debug: bool,
    /// A short wait before the reply, the typing animation in local chat (FakeTypeDelay).
    pub typing: bool,
}

impl std::fmt::Debug for Settings {
    // never prints the key
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatGpt")
            .field("api_key_set", &self.api_key.is_some())
            .field("organization_set", &self.organization.is_some())
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("history", &self.history)
            .field("history_timeout", &self.history_timeout)
            .field("localchat", &self.localchat)
            .field("im", &self.im)
            .field("im_friends_only", &self.im_friends_only)
            .field("group", &self.group)
            .field("typing", &self.typing)
            .finish_non_exhaustive()
    }
}

/// SecondBot's InRange, its arguments in the order it meant.
pub fn in_range(value: i64, min: i64, max: i64) -> i64 {
    value.clamp(min, max)
}

/// Where the chat completions are asked. "openai" is OpenAI's API; another provider is the
/// address the library is given as its BaseDomain, under which it asks `v1/chat/completions`.
/// An address that already ends with `/v1` or `/chat/completions` is taken as it is meant.
pub fn endpoint(provider: &str) -> Result<String, String> {
    let provider = provider.trim();
    let base = if provider.eq_ignore_ascii_case(OPENAI) {
        OPENAI_BASE
    } else if provider.starts_with("https://") || provider.starts_with("http://") {
        provider.trim_end_matches('/')
    } else {
        return Err("\"openai\", or an http(s) address".into());
    };
    Ok(if base.ends_with("/chat/completions") {
        base.to_owned()
    } else if base.ends_with("/v1") {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    })
}

/// SecondBot's OpenAIModels.GetModel, for OpenAI only: its short names for OpenAI's models.
pub fn model_name(provider: &str, model: &str) -> String {
    if !provider.eq_ignore_ascii_case(OPENAI) {
        return model.to_owned();
    }
    match model {
        "gpt-4-mini" => "gpt-4o-mini".to_owned(),
        other => other.to_owned(),
    }
}

/// A conversation: local chat, an avatar's IMs, a group's chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Window {
    Local,
    Avatar(Uuid),
    Group(Uuid),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

struct Conversation {
    lines: Vec<(Role, String)>,
    /// When it was last used, in unix seconds.
    last: u64,
}

/// The conversations and their cooldowns (SecondBot's chatHistoryAI, ChatHistoryLastAccessed
/// and ChatRateLimiter).
pub struct Conversations {
    windows: HashMap<Window, Conversation>,
    /// When each conversation may ask again, as SecondBot keeps it: the time of its last
    /// question plus one second.
    cooldowns: HashMap<Window, u64>,
    size: usize,
    timeout: u64,
}

impl Conversations {
    pub fn new(size: usize, timeout: Duration) -> Conversations {
        Conversations {
            windows: HashMap::new(),
            cooldowns: HashMap::new(),
            size,
            timeout: timeout.as_secs(),
        }
    }

    /// SecondBot's upkeep: conversations quiet for longer than the timeout are forgotten, their
    /// cooldowns with them.
    fn expire(&mut self, now: u64) {
        let timeout = self.timeout;
        let old: Vec<Window> = self
            .windows
            .iter()
            .filter(|(_, c)| now.saturating_sub(c.last) > timeout)
            .map(|(w, _)| *w)
            .collect();
        for w in old {
            self.windows.remove(&w);
            self.cooldowns.remove(&w);
        }
        // a cooldown whose conversation is gone (the oldest forgotten) goes once it is over
        let windows = &self.windows;
        self.cooldowns
            .retain(|w, last| windows.contains_key(w) || now.saturating_sub(*last) <= MAX_COOLDOWN);
    }

    /// SecondBot's rate limiter: refused while `now - last <= cooldown`; when allowed, the time
    /// is marked (one second ahead, as SecondBot's).
    pub fn may_ask(&mut self, w: Window, cooldown: u64, now: u64) -> bool {
        self.expire(now);
        let last = self.cooldowns.get(&w).copied().unwrap_or(0);
        if now.saturating_sub(last) <= cooldown {
            return false;
        }
        self.cooldowns.insert(w, now + 1);
        true
    }

    /// Adds a line to the conversation, started with `system` when new, and gives the lines to
    /// send: the system line, then the last `size` lines.
    pub fn add(
        &mut self,
        w: Window,
        system: impl FnOnce() -> String,
        role: Role,
        line: String,
        now: u64,
    ) -> Vec<(Role, String)> {
        if !self.windows.contains_key(&w) && self.windows.len() >= MAX_WINDOWS {
            let oldest = self
                .windows
                .iter()
                .min_by_key(|(_, c)| c.last)
                .map(|(w, _)| *w);
            if let Some(oldest) = oldest {
                self.windows.remove(&oldest);
            }
        }
        let c = self.windows.entry(w).or_insert_with(|| Conversation {
            lines: vec![(Role::System, system())],
            last: now,
        });
        c.last = now;
        c.lines.push((role, line));
        // BasicSaveHistory: the system line stays, the oldest after it go
        while c.lines.len() > self.size + 1 {
            c.lines.remove(1);
        }
        c.lines.clone()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.windows.len()
    }
}

/// The request's JSON: the model and the lines.
pub fn request_body(model: &str, lines: &[(Role, String)]) -> String {
    let messages: Vec<serde_json::Value> = lines
        .iter()
        .map(|(role, content)| serde_json::json!({ "role": role.as_str(), "content": content }))
        .collect();
    serde_json::json!({ "model": model, "messages": messages }).to_string()
}

/// The reply in a chat completion's answer (`choices[0].message.content`), or why there is none:
/// the provider's error message when it sends one (`error.message`), else the HTTP status.
pub fn read_reply(status: u16, body: &[u8]) -> Result<String, String> {
    let v: Option<serde_json::Value> = serde_json::from_slice(body).ok();
    let error = v.as_ref().and_then(|v| {
        let e = &v["error"];
        e["message"]
            .as_str()
            .or_else(|| e.as_str())
            .map(str::to_owned)
    });
    if !(200..300).contains(&status) {
        return Err(match error {
            Some(m) => format!("HTTP {status}: {m}"),
            None => format!("HTTP {status}"),
        });
    }
    if let Some(m) = error {
        return Err(m);
    }
    let Some(v) = v else {
        return Err("the answer is not JSON".into());
    };
    let reply = v["choices"][0]["message"]["content"]
        .as_str()
        .map(str::trim)
        .unwrap_or_default();
    if reply.is_empty() {
        return Err("The reply was empty".into());
    }
    Ok(reply.to_owned())
}

/// The key never reaches the log, even when a provider quotes it back.
pub fn redact(text: &str, key: Option<&str>) -> String {
    let mut out = match key.filter(|k| k.len() >= 4) {
        Some(k) => text.replace(k, "[redacted]"),
        None => text.to_owned(),
    };
    if out.len() > MAX_ERROR {
        let cut = cut(&out, MAX_ERROR).len();
        out.truncate(cut);
        out.push('…');
    }
    out
}

/// The text, no longer than `max` bytes, cut between characters.
pub fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// SecondBot's EstimateTypingTime (200 characters a minute, 3 s at most, up to 40 % less at
/// random), then held between 1 and 3 s. `random` is in 0..1.
pub fn typing_time(reply: &str, random: f64) -> Duration {
    let base = (reply.chars().count() as f64 / 200.0 * 60.0).min(3.0);
    let estimate = (base - random * 0.4 * base).max(0.0);
    Duration::from_secs_f64(estimate.clamp(1.0, 3.0))
}

fn random() -> f64 {
    use std::hash::{BuildHasher, Hasher};
    let n = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    (n % 10_000) as f64 / 10_000.0
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The system line a conversation starts with (GetHistory).
pub fn system_line(name: &str, prompt: &str, w: Window, talking_to: &str) -> String {
    let who = match w {
        Window::Avatar(_) => talking_to,
        Window::Group(_) => "a group of people",
        Window::Local => "people in a public place",
    };
    format!("You are {name}, {prompt}, you are talking to {who}.")
}

struct Service {
    client: Client,
    settings: Settings,
    http: reqwest::Client,
    conversations: Mutex<Conversations>,
    in_flight: AtomicUsize,
}

/// Something heard that may be answered.
struct Heard {
    window: Window,
    cooldown: u64,
    name: String,
    message: String,
}

impl Service {
    fn trace(&self, text: &str) {
        if self.settings.debug {
            log::info!("AI: {text}");
        } else {
            log::debug!("AI: {text}");
        }
    }

    /// What to answer, if anything: SecondBot's BotLocalchat and BotImMessage, and the group's
    /// chat.
    fn heard(&self, event: Event) -> Option<Heard> {
        let s = &self.settings;
        let me = || self.client.state().agent_id;
        match event {
            Event::Chat(c) if s.localchat => {
                let spoken = matches!(
                    c.chat_type,
                    chat_type::WHISPER | chat_type::SAY | chat_type::SHOUT | REGION_SAY_TO
                );
                // avatars only: objects (0 system, 2 object) and the bot itself are not answered
                if !spoken
                    || c.source_type != 1
                    || c.source_id == me()
                    || BLOCKED.contains(&c.from_name.to_lowercase().as_str())
                    || c.message.trim().is_empty()
                {
                    return None;
                }
                Some(Heard {
                    window: Window::Local,
                    cooldown: s.localchat_cooldown,
                    name: c.from_name,
                    message: c.message,
                })
            }
            Event::InstantMessage(im) if s.im => {
                let state = self.client.state();
                // an avatar's IM, or a conference's line (a group's goes to its own chat)
                let from_avatar = im.dialog == dialog::NOTHING_SPECIAL
                    || (im.dialog == dialog::SESSION_SEND
                        && !state.groups.iter().any(|g| g.id == im.id));
                if !from_avatar
                    || im.from_agent_id == state.agent_id
                    || im.from_agent_id.is_nil()
                    || im.message.trim().is_empty()
                {
                    return None;
                }
                if s.im_friends_only && !state.friends.contains(&im.from_agent_id) {
                    return None;
                }
                // a command for the bot, maybe signed, is no talk to send away
                if unpack(&im.message).is_some_and(|u| crate::commands::known(&u.command).is_some())
                {
                    return None;
                }
                Some(Heard {
                    window: Window::Avatar(im.from_agent_id),
                    cooldown: s.im_cooldown,
                    name: im.from_name,
                    message: im.message,
                })
            }
            Event::GroupChat {
                group,
                from_id,
                from_name,
                message,
            } if s.group == Some(group) => {
                if from_id == me() || message.trim().is_empty() {
                    return None;
                }
                Some(Heard {
                    window: Window::Group(group),
                    cooldown: s.group_cooldown,
                    name: from_name,
                    message,
                })
            }
            _ => None,
        }
    }

    /// SecondBot's GetAiReply.
    async fn answer(self: Arc<Self>, h: Heard) {
        self.trace(&format!("Request from {}: {}", h.name, h.message));
        let now = now();
        let allowed = self
            .conversations
            .lock()
            .map(|mut c| c.may_ask(h.window, h.cooldown, now))
            .unwrap_or(false);
        if !allowed {
            self.trace(&format!(
                "Skipped, replying too fast (rate limit {}s)",
                h.cooldown
            ));
            return;
        }
        if self.in_flight.fetch_add(1, Ordering::SeqCst) >= MAX_IN_FLIGHT {
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            log::warn!(
                "AI: skipped a request from {}: {MAX_IN_FLIGHT} already asked",
                h.name
            );
            return;
        }
        let result = self.ask(&h, now).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        match result {
            Ok(reply) => self.say(&h, &reply).await,
            Err(e) => log::warn!(
                "AI: no reply to {}: {}",
                h.name,
                redact(&e, self.settings.api_key.as_deref())
            ),
        }
    }

    /// The question added to its conversation, asked; the reply added too.
    async fn ask(&self, h: &Heard, now: u64) -> Result<String, String> {
        let s = &self.settings;
        let name = s.name.clone().unwrap_or_else(|| {
            let full = self.client.state().name;
            full.split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned()
        });
        let lines = self
            .conversations
            .lock()
            .map_err(|_| "the conversations are not readable".to_owned())?
            .add(
                h.window,
                || system_line(&name, &s.prompt, h.window, &h.name),
                Role::User,
                format!("{} says {}", h.name, h.message),
                now,
            );
        self.trace(&format!("Asking {} ({})", s.provider, s.model));
        let mut req = self
            .http
            .post(&s.endpoint)
            .header("Content-Type", "application/json")
            .body(request_body(&model_name(&s.provider, &s.model), &lines));
        if let Some(key) = &s.api_key {
            req = req.bearer_auth(key);
        }
        if let Some(org) = &s.organization {
            req = req.header("OpenAI-Organization", org);
        }
        let mut resp = req.send().await.map_err(describe)?;
        let status = resp.status().as_u16();
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(describe)? {
            if body.len() + chunk.len() > MAX_BODY {
                return Err(format!("the answer is larger than {MAX_BODY} bytes"));
            }
            body.extend_from_slice(&chunk);
        }
        let reply = read_reply(status, &body)?;
        let reply = cut(&reply, MAX_REPLY).to_owned();
        self.trace(&format!("Reply: {reply}"));
        if let Ok(mut c) = self.conversations.lock() {
            c.add(
                h.window,
                || system_line(&name, &s.prompt, h.window, &h.name),
                Role::Assistant,
                reply.clone(),
                self::now(),
            );
        }
        Ok(reply)
    }

    /// The reply, said where the question came from: never run as a command.
    async fn say(&self, h: &Heard, reply: &str) {
        let local = h.window == Window::Local;
        if self.settings.typing {
            if local {
                let _ = self.client.animate(vec![(TYPE_ANIMATION, true)]).await;
            }
            tokio::time::sleep(typing_time(reply, random())).await;
        }
        let sent = match h.window {
            Window::Local => {
                let sent = self.client.chat(reply, 0, chat_type::SAY).await;
                if self.settings.typing {
                    let _ = self.client.animate(vec![(TYPE_ANIMATION, false)]).await;
                }
                sent
            }
            Window::Avatar(id) => self.client.im(id, reply).await,
            Window::Group(id) => self.client.group_chat_send(id, reply).await,
        };
        match sent {
            Ok(()) => log::info!("AI: replied to {}", h.name),
            Err(e) => log::warn!("AI: the reply to {} was not sent: {e}", h.name),
        }
    }
}

/// An error and its causes, the URL left out.
fn describe(e: reqwest::Error) -> String {
    if e.is_timeout() {
        return format!("no answer within {} s", TIMEOUT.as_secs());
    }
    let e = e.without_url();
    let mut text = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        text.push_str(": ");
        text.push_str(&s.to_string());
        source = s.source();
    }
    text
}

pub async fn run(client: Client, mut events: Receiver<Event>, settings: Option<Settings>) {
    let Some(settings) = settings else {
        return;
    };
    // no redirects: the key goes to the endpoint given, nowhere else
    let http = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TIMEOUT)
        .user_agent(format!("Ferrobot/{}", ferro_client::login::version()))
        .build()
    {
        Ok(http) => http,
        Err(e) => {
            log::error!("AI: no HTTP client: {}", describe(e));
            return;
        }
    };
    log::info!(
        "AI replies: {} ({}){}{}{}",
        settings.provider,
        settings.model,
        if settings.localchat {
            ", local chat"
        } else {
            ""
        },
        if settings.im { ", IMs" } else { "" },
        if settings.group.is_some() {
            ", group chat"
        } else {
            ""
        },
    );
    let service = Arc::new(Service {
        conversations: Mutex::new(Conversations::new(
            settings.history,
            settings.history_timeout,
        )),
        client,
        settings,
        http,
        in_flight: AtomicUsize::new(0),
    });
    loop {
        match events.recv().await {
            Ok(event) => {
                if let Some(h) = service.heard(event) {
                    tokio::spawn(service.clone().answer(h));
                }
            }
            Err(RecvError::Lagged(n)) => log::warn!("AI: {n} events missed"),
            Err(RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints() {
        assert_eq!(
            endpoint("openai").as_deref(),
            Ok("https://api.openai.com/v1/chat/completions")
        );
        // the library's BaseDomain, with or without its last slash
        assert_eq!(
            endpoint("https://openrouter.example/api/").as_deref(),
            Ok("https://openrouter.example/api/v1/chat/completions")
        );
        assert_eq!(
            endpoint("http://127.0.0.1:11434").as_deref(),
            Ok("http://127.0.0.1:11434/v1/chat/completions")
        );
        assert_eq!(
            endpoint("https://provider.example/v1").as_deref(),
            Ok("https://provider.example/v1/chat/completions")
        );
        assert_eq!(
            endpoint("https://provider.example/x/chat/completions").as_deref(),
            Ok("https://provider.example/x/chat/completions")
        );
        assert!(endpoint("provider.example").is_err());
        assert!(endpoint("ftp://provider.example").is_err());
    }

    #[test]
    fn models() {
        assert_eq!(model_name("openai", "gpt-4-mini"), "gpt-4o-mini");
        assert_eq!(model_name("openai", "gpt-3.5-turbo"), "gpt-3.5-turbo");
        assert_eq!(
            model_name("https://provider.example", "gpt-4-mini"),
            "gpt-4-mini"
        );
    }

    #[test]
    fn ranges() {
        assert_eq!(in_range(5, 3, 10), 5);
        assert_eq!(in_range(0, 3, 10), 3);
        assert_eq!(in_range(50, 1, 10), 10);
    }

    #[test]
    fn cooldowns() {
        let mut c = Conversations::new(5, Duration::from_secs(15 * 60));
        let a = Window::Avatar(Uuid::from_u128(1));
        assert!(c.may_ask(a, 3, 1_000));
        // marked one second ahead: refused while now - 1001 <= 3
        assert!(!c.may_ask(a, 3, 1_001));
        assert!(!c.may_ask(a, 3, 1_004));
        assert!(c.may_ask(a, 3, 1_005));
        // another window has its own
        assert!(c.may_ask(Window::Local, 3, 1_005));
        assert!(c.may_ask(Window::Avatar(Uuid::from_u128(2)), 3, 1_005));
    }

    #[test]
    fn the_history_keeps_the_system_line_and_the_last_lines() {
        let mut c = Conversations::new(3, Duration::from_secs(60));
        let w = Window::Local;
        let sys =
            || "You are Bot, a horse, you are talking to people in a public place.".to_owned();
        let first = c.add(w, sys, Role::User, "A says 1".into(), 10);
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].0, Role::System);
        for n in 2..=5 {
            c.add(
                w,
                || unreachable!("started"),
                Role::User,
                format!("A says {n}"),
                10,
            );
        }
        let lines = c.add(w, sys, Role::Assistant, "neigh".into(), 10);
        assert_eq!(
            lines,
            vec![
                (Role::System, sys()),
                (Role::User, "A says 4".into()),
                (Role::User, "A says 5".into()),
                (Role::Assistant, "neigh".into()),
            ]
        );
    }

    #[test]
    fn quiet_conversations_are_forgotten() {
        let mut c = Conversations::new(5, Duration::from_secs(60));
        let w = Window::Avatar(Uuid::from_u128(7));
        assert!(c.may_ask(w, 3, 100));
        c.add(w, || "sys".into(), Role::User, "x".into(), 100);
        assert_eq!(c.len(), 1);
        assert!(!c.may_ask(w, 3, 102));
        // 61 s later: forgotten, and its cooldown with it
        assert!(c.may_ask(w, 3, 161));
        assert_eq!(c.len(), 0);
        let lines = c.add(w, || "new".into(), Role::User, "y".into(), 161);
        assert_eq!(lines[0].1, "new");
    }

    #[test]
    fn at_most_so_many_conversations() {
        let mut c = Conversations::new(5, Duration::from_secs(3600));
        for n in 0..(MAX_WINDOWS as u128 + 10) {
            c.add(
                Window::Avatar(Uuid::from_u128(n)),
                || "sys".into(),
                Role::User,
                "x".into(),
                1_000 + n as u64,
            );
        }
        assert_eq!(c.len(), MAX_WINDOWS);
    }

    #[test]
    fn system_lines() {
        let a = Window::Avatar(Uuid::from_u128(1));
        assert_eq!(
            system_line("Bot", "be kind", a, "Some Resident"),
            "You are Bot, be kind, you are talking to Some Resident."
        );
        assert_eq!(
            system_line("Bot", "be kind", Window::Group(Uuid::nil()), "x"),
            "You are Bot, be kind, you are talking to a group of people."
        );
        assert_eq!(
            system_line("Bot", "be kind", Window::Local, "x"),
            "You are Bot, be kind, you are talking to people in a public place."
        );
    }

    #[test]
    fn the_request() {
        let body = request_body(
            "m",
            &[
                (Role::System, "s".into()),
                (Role::User, "A says \"hi\"".into()),
            ],
        );
        let v: serde_json::Value = serde_json::from_str(&body).expect("json");
        assert_eq!(v["model"], "m");
        assert_eq!(v["messages"][0]["role"], "system");
        assert_eq!(v["messages"][1]["role"], "user");
        assert_eq!(v["messages"][1]["content"], "A says \"hi\"");
    }

    #[test]
    fn replies_and_errors() {
        let ok =
            br#"{"choices":[{"index":0,"message":{"role":"assistant","content":" Neigh. "}}]}"#;
        assert_eq!(read_reply(200, ok), Ok("Neigh.".into()));
        let empty = br#"{"choices":[{"message":{"role":"assistant","content":""}}]}"#;
        assert_eq!(read_reply(200, empty), Err("The reply was empty".into()));
        assert_eq!(read_reply(200, b"{}"), Err("The reply was empty".into()));
        assert!(read_reply(200, b"not json").is_err());
        let refused =
            br#"{"error":{"message":"Incorrect API key","type":"invalid_request_error"}}"#;
        assert_eq!(
            read_reply(401, refused),
            Err("HTTP 401: Incorrect API key".into())
        );
        // OpenRouter's error code is a number: the message is still read
        let router = br#"{"error":{"code":429,"message":"Rate limited"}}"#;
        assert_eq!(
            read_reply(429, router),
            Err("HTTP 429: Rate limited".into())
        );
        // an error in a 200
        assert_eq!(read_reply(200, router), Err("Rate limited".into()));
        assert_eq!(read_reply(502, b"<html>"), Err("HTTP 502".into()));
    }

    #[test]
    fn the_key_never_shows() {
        let key = "sk-test-0123456789";
        assert_eq!(
            redact("Incorrect API key provided: sk-test-0123456789.", Some(key)),
            "Incorrect API key provided: [redacted]."
        );
        assert_eq!(redact("no key here", None), "no key here");
        let long = "é".repeat(400);
        let r = redact(&long, Some(key));
        assert!(r.len() <= MAX_ERROR + '…'.len_utf8());
    }

    #[test]
    fn replies_are_cut_between_characters() {
        assert_eq!(cut("hello", 10), "hello");
        assert_eq!(cut("hello", 3), "hel");
        // "é" is two bytes: never cut in its middle
        assert_eq!(cut("éé", 3), "é");
        let long = "a".repeat(5000);
        assert_eq!(cut(&long, MAX_REPLY).len(), MAX_REPLY);
    }

    #[test]
    fn typing_takes_one_to_three_seconds() {
        assert_eq!(typing_time("hi", 0.0), Duration::from_secs(1));
        assert_eq!(typing_time(&"a".repeat(500), 0.0), Duration::from_secs(3));
        // 20 characters: 6 s, held to 3; with the most taken off, 1.8 s
        let t = typing_time(&"a".repeat(20), 1.0);
        assert!((t.as_secs_f64() - 1.8).abs() < 1e-9, "{t:?}");
        let r = random();
        assert!((0.0..1.0).contains(&r));
    }

    #[test]
    fn settings_never_print_the_key() {
        let s = Settings {
            api_key: Some("sk-secret".into()),
            organization: Some("org-secret".into()),
            provider: "openai".into(),
            endpoint: endpoint("openai").expect("endpoint"),
            model: DEFAULT_MODEL.into(),
            prompt: DEFAULT_PROMPT.into(),
            name: None,
            history: 5,
            history_timeout: Duration::from_secs(900),
            localchat: true,
            localchat_cooldown: 3,
            im: false,
            im_friends_only: false,
            im_cooldown: 3,
            group: None,
            group_cooldown: 3,
            debug: false,
            typing: true,
        };
        let printed = format!("{s:?}");
        assert!(!printed.contains("sk-secret"), "{printed}");
        assert!(!printed.contains("org-secret"), "{printed}");
    }
}
