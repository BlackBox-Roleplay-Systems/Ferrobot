//! Discord, as SecondBot's DiscordService: the bot on a Discord server, with a `bot` category
//! (`status`, `commands`, `localchat`), an `im` category (a channel per avatar) and a `group`
//! category (a channel per group). What the bot hears in Second Life goes to those channels;
//! what a Discord master writes there goes back (relay.rs). The commands of SecondBot's Discord
//! class work on the server through the same connection (commands/discord.rs).
//!
//! Deliberate differences: only Discord masters (FERRO_DISCORD_MASTERS) speak through the bot,
//! use the channels' `!` commands or the `commands` channel (SecondBot takes them from anyone on
//! the server); the `commands` channel follows FERRO_DISCORD_COMMANDS (SecondBot's
//! AllowDiscordCommands, which it never reads); no Discord voice (the container carries no audio
//! library).

pub mod relay;

use crate::services::control::Spec;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use twilight_gateway::{
    CloseFrame, ConfigBuilder, Event, EventTypeFlags, Intents, MessageSender, Shard, ShardId,
    StreamExt,
};
use twilight_http::Client as Http;
use twilight_model::channel::{Channel, ChannelType};
use twilight_model::gateway::payload::outgoing::UpdatePresence;
use twilight_model::gateway::presence::{ActivityType, MinimalActivity, Status};
use twilight_model::id::Id;
use twilight_model::id::marker::{ChannelMarker, GuildMarker, MessageMarker, UserMarker};

/// SecondBot's Discord settings.
#[derive(Clone)]
pub struct Settings {
    /// The bot's token: a secret, never logged.
    pub token: String,
    pub server: u64,
    /// The `commands` channel (SecondBot's AllowDiscordCommands).
    pub commands: bool,
    /// The Discord users the bot takes words and commands from.
    pub masters: Vec<u64>,
    /// Lines sent to Second Life without the Discord user's name (hideChatterName).
    pub hide_names: bool,
    /// The bot's channels emptied when it connects, `commands` excepted (ClearChatOnConnect).
    pub clear_on_connect: bool,
    /// SecondBot's interaction: a message starting with its word, in its channel, goes to an
    /// HTTP address.
    pub interaction: Option<Interaction>,
    /// For tests: `host:port` of a stand-in Discord, its API over plain HTTP and its gateway over
    /// plain WebSocket.
    pub stand_in: Option<String>,
}

impl std::fmt::Debug for Settings {
    // never prints the token
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Discord")
            .field("server", &self.server)
            .field("commands", &self.commands)
            .field("masters", &self.masters.len())
            .field("hide_names", &self.hide_names)
            .field("clear_on_connect", &self.clear_on_connect)
            .field("interaction", &self.interaction.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct Interaction {
    /// The word a message starts with (InteractionCommandName).
    pub name: String,
    pub url: String,
    pub channel: u64,
}

/// The bot's three categories.
pub const BOT: &str = "bot";
pub const IM: &str = "im";
pub const GROUP: &str = "group";

/// A text channel of the server, as the bot knows it.
#[derive(Debug, Clone)]
pub struct Known {
    pub id: Id<ChannelMarker>,
    pub name: String,
    /// Its category's name, when it is one of the bot's.
    pub category: Option<String>,
    pub topic: String,
}

#[derive(Default)]
struct State {
    ready: bool,
    me: Option<Id<UserMarker>>,
    categories: HashMap<String, Id<ChannelMarker>>,
    channels: HashMap<Id<ChannelMarker>, Known>,
}

/// The connection to Discord, shared by the relay and the commands.
pub struct Discord {
    pub http: Http,
    pub settings: Settings,
    /// The shared secret, to sign the interaction's posts. Never logged.
    pub secret: Option<String>,
    state: Mutex<State>,
    sender: Mutex<Option<MessageSender>>,
}

/// SecondBot's GetChannelName: lower case, letters, digits and dashes for spaces.
pub fn channel_name(input: &str) -> String {
    let lower = input.replace('-', "").to_lowercase();
    let kept: String = lower
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == ' ')
        .collect();
    let mut name = kept.replace(' ', "-");
    while name.contains("--") {
        name = name.replace("--", "-");
    }
    name
}

/// Discord's limit on a message (ReplyToMessage cuts at 1900 and adds "...").
pub fn fit(text: &str) -> String {
    if text.chars().count() <= 2000 {
        return text.to_owned();
    }
    let cut: String = text.chars().take(1900).collect();
    format!("{cut}...")
}

impl Discord {
    pub fn new(settings: Settings, secret: Option<String>) -> Arc<Discord> {
        let mut http = Http::builder().token(settings.token.clone());
        if let Some(host) = &settings.stand_in {
            http = http.proxy(host.clone(), true);
        }
        Arc::new(Discord {
            http: http.build(),
            settings,
            secret,
            state: Mutex::new(State::default()),
            sender: Mutex::new(None),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn server(&self) -> Id<GuildMarker> {
        Id::new(self.settings.server)
    }

    /// Connected, and the channels set up (SecondBot's GetReadyForDiscordActions).
    pub fn ready(&self) -> bool {
        self.state().ready
    }

    pub fn me(&self) -> Option<Id<UserMarker>> {
        self.state().me
    }

    pub fn is_master(&self, user: Id<UserMarker>) -> bool {
        self.settings.masters.contains(&user.get())
    }

    /// A channel the bot knows, by id.
    pub fn known(&self, id: Id<ChannelMarker>) -> Option<Known> {
        self.state().channels.get(&id).cloned()
    }

    fn remember(&self, c: &Channel) -> Option<Known> {
        if c.kind != ChannelType::GuildText {
            return None;
        }
        let mut st = self.state();
        let category = c.parent_id.and_then(|p| {
            st.categories
                .iter()
                .find(|(_, id)| **id == p)
                .map(|(n, _)| n.clone())
        });
        let k = Known {
            id: c.id,
            name: c.name.clone().unwrap_or_default(),
            category,
            topic: c.topic.clone().unwrap_or_default(),
        };
        st.channels.insert(c.id, k.clone());
        Some(k)
    }

    pub fn forget(&self, id: Id<ChannelMarker>) {
        self.state().channels.remove(&id);
    }

    /// A channel the bot has not seen yet (one a user made): asked of Discord once.
    pub async fn learn(&self, id: Id<ChannelMarker>) -> Option<Known> {
        if let Some(k) = self.known(id) {
            return Some(k);
        }
        let c = self.http.channel(id).await.ok()?.model().await.ok()?;
        self.remember(&c)
    }

    /// SecondBot's DoServerChannelSetup: the categories and the `bot` channels, made when
    /// missing; the bot's channels emptied when asked (`commands` kept).
    async fn set_up(&self) -> Result<(), String> {
        let server = self.server();
        let channels = self
            .http
            .guild_channels(server)
            .await
            .map_err(|e| e.to_string())?
            .models()
            .await
            .map_err(|e| e.to_string())?;
        for wanted in [BOT, IM, GROUP] {
            let found = channels
                .iter()
                .find(|c| c.kind == ChannelType::GuildCategory && c.name.as_deref() == Some(wanted))
                .map(|c| c.id);
            let id = match found {
                Some(id) => id,
                None => {
                    self.http
                        .create_guild_channel(server, wanted)
                        .kind(ChannelType::GuildCategory)
                        .await
                        .map_err(|e| e.to_string())?
                        .model()
                        .await
                        .map_err(|e| e.to_string())?
                        .id
                }
            };
            self.state().categories.insert(wanted.to_owned(), id);
        }
        self.state().channels.clear();
        for c in &channels {
            self.remember(c);
        }
        for (name, topic) in [
            ("status", relay::STATUS_TOPIC),
            (
                "commands",
                "Send a command to the bot as if you are a master",
            ),
            ("localchat", relay::LOCAL_TOPIC),
        ] {
            let existing = channels
                .iter()
                .find(|c| c.kind == ChannelType::GuildText && c.name.as_deref() == Some(name))
                .map(|c| c.id);
            match existing {
                Some(id) => {
                    if self.settings.clear_on_connect && name != "commands" {
                        self.clean(id).await;
                    }
                }
                None => {
                    self.make_channel(name, topic, BOT).await?;
                }
            }
        }
        Ok(())
    }

    async fn make_channel(
        &self,
        name: &str,
        topic: &str,
        category: &str,
    ) -> Result<Id<ChannelMarker>, String> {
        let parent = self.state().categories.get(category).copied();
        let mut req = self
            .http
            .create_guild_channel(self.server(), name)
            .kind(ChannelType::GuildText)
            .topic(topic);
        if let Some(p) = parent {
            req = req.parent_id(p);
        }
        let c = req
            .await
            .map_err(|e| e.to_string())?
            .model()
            .await
            .map_err(|e| e.to_string())?;
        self.remember(&c);
        Ok(c.id)
    }

    /// SecondBot's GetChannel: the channel of that name in that category, made when missing.
    pub async fn channel(
        &self,
        name: &str,
        topic: &str,
        category: &str,
    ) -> Option<Id<ChannelMarker>> {
        let name = channel_name(name);
        if name.is_empty() {
            return None;
        }
        let found = self
            .state()
            .channels
            .values()
            .find(|k| k.name == name && k.category.as_deref() == Some(category))
            .map(|k| k.id);
        if found.is_some() {
            return found;
        }
        match self.make_channel(&name, topic, category).await {
            Ok(id) => Some(id),
            Err(e) => {
                log::warn!("discord: could not make the channel {name}: {e}");
                None
            }
        }
    }

    pub async fn post(&self, channel: Id<ChannelMarker>, text: &str) -> Option<Id<MessageMarker>> {
        let text = fit(text);
        match self.http.create_message(channel).content(&text).await {
            Ok(r) => r.model().await.ok().map(|m| m.id),
            Err(e) => {
                log::warn!("discord: a message was not posted: {e}");
                None
            }
        }
    }

    /// SecondBot's CleanChannel: the last 100 messages, those of the last 14 days in one go,
    /// older ones one by one.
    pub async fn clean(&self, channel: Id<ChannelMarker>) {
        let Ok(r) = self.http.channel_messages(channel).limit(100).await else {
            return;
        };
        let Ok(messages) = r.models().await else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let (recent, old): (Vec<_>, Vec<_>) = messages
            .iter()
            .partition(|m| now - m.timestamp.as_secs() <= 14 * 86_400);
        let ids: Vec<Id<MessageMarker>> = recent.iter().map(|m| m.id).collect();
        match ids.len() {
            0 => {}
            1 => {
                let _ = self.http.delete_message(channel, ids[0]).await;
            }
            _ => {
                let _ = self.http.delete_messages(channel, &ids).await;
            }
        }
        for m in old {
            let _ = self.http.delete_message(channel, m.id).await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// The bot's activity: playing its region (SecondBot's SetGameAsync).
    pub fn presence(&self, text: &str, listening: bool) {
        let kind = if listening {
            ActivityType::Listening
        } else {
            ActivityType::Playing
        };
        let activity = MinimalActivity {
            kind,
            name: text.to_owned(),
            url: None,
        };
        let Ok(update) = UpdatePresence::new(vec![activity.into()], false, None, Status::Online)
        else {
            return;
        };
        if let Some(sender) = self
            .sender
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            let _ = sender.command(&update);
        }
    }
}

/// The connection while DiscordService runs, for the commands; none while it is stopped or off.
pub type Slot = std::sync::RwLock<Option<Arc<Discord>>>;

/// The connection, while DiscordService runs.
pub fn connection(slot: &Slot) -> Option<Arc<Discord>> {
    slot.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// DiscordService's settings (SecondBot's DiscordConfig, `discord_…`). Its token is a secret:
/// never changed by command.
const SETTINGS: &[&str] = &[
    "Enabled",
    "ServerID",
    "AllowDiscordCommands",
    "InteractionEnabled",
    "InteractionCommandName",
    "InteractionHttpTarget",
    "InteractionChannelNumber",
    "hideChatterName",
    "ClearChatOnConnect",
];

/// DiscordService, for the service control (services/control.rs). Stopped, the bot leaves the
/// gateway and the Discord commands answer that Discord is not ready.
pub fn spec(ctx: crate::commands::Ctx, secret: Option<String>) -> Spec {
    Spec::new(
        "DiscordService",
        "discord",
        SETTINGS,
        crate::config::discord,
        move |settings| match settings {
            Some(s) => spawn(ctx.clone(), Discord::new(s, secret.clone())),
            None => Vec::new(),
        },
    )
}

/// Takes the connection out of the slot when its gateway task ends or is stopped (unless a new
/// one took its place).
struct Listed(Arc<Slot>, Arc<Discord>);

impl Drop for Listed {
    fn drop(&mut self) {
        let mut slot = self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.as_ref().is_some_and(|d| Arc::ptr_eq(d, &self.1)) {
            *slot = None;
        }
    }
}

/// The relay from Second Life (subscribed now) and the gateway; the connection in the slot.
fn spawn(ctx: crate::commands::Ctx, discord: Arc<Discord>) -> Vec<tokio::task::JoinHandle<()>> {
    log::info!("discord: server {}", discord.settings.server);
    *ctx.discord
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(discord.clone());
    let listed = Listed(ctx.discord.clone(), discord.clone());
    let from_world = tokio::spawn(relay::from_world(
        ctx.client.clone(),
        discord.clone(),
        ctx.client.subscribe(),
    ));
    let gateway = tokio::spawn(async move {
        let _listed = listed;
        run(ctx, discord).await;
    });
    vec![from_world, gateway]
}

/// The gateway: connected, the channels set up, then the messages to the relay. Twilight
/// reconnects by itself; a fatal close (a bad token, intents not allowed) stops the service.
async fn run(ctx: crate::commands::Ctx, discord: Arc<Discord>) {
    // MESSAGE_CONTENT is privileged: it is turned on in the Discord application's settings
    let intents = Intents::GUILDS | Intents::GUILD_MESSAGES | Intents::MESSAGE_CONTENT;
    let mut config = ConfigBuilder::new(discord.settings.token.clone(), intents);
    if let Some(host) = &discord.settings.stand_in {
        config = config.proxy_url(format!("ws://{host}/gateway"));
    }
    let mut shard = Shard::with_config(ShardId::ONE, config.build());
    *discord
        .sender
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(shard.sender());
    let wanted = EventTypeFlags::READY | EventTypeFlags::MESSAGE_CREATE;
    while let Some(item) = shard.next_event(wanted).await {
        let event = match item {
            Ok(e) => e,
            Err(e) => {
                log::warn!("discord: {e}");
                continue;
            }
        };
        match event {
            Event::Ready(r) => {
                discord.state().me = Some(r.user.id);
                log::info!("discord: connected");
                match discord.set_up().await {
                    Ok(()) => {
                        discord.state().ready = true;
                        log::info!("discord: channels ready");
                        relay::on_ready(&ctx, &discord).await;
                    }
                    Err(e) => log::warn!("discord: the channels could not be set up: {e}"),
                }
            }
            Event::MessageCreate(m) => {
                let (ctx, discord) = (ctx.clone(), discord.clone());
                tokio::spawn(async move { relay::from_discord(&ctx, &discord, m.message).await });
            }
            Event::GatewayClose(Some(CloseFrame { code, .. })) if is_fatal(code) => {
                log::error!(
                    "discord: the gateway refused the bot (close code {code}): check its token and its intents"
                );
                discord.state().ready = false;
                return;
            }
            Event::GatewayClose(_) => {
                discord.state().ready = false;
                log::info!("discord: disconnected, connecting again");
            }
            _ => {}
        }
    }
}

/// Close codes after which connecting again cannot help (an invalid token, intents refused…).
fn is_fatal(code: u16) -> bool {
    matches!(code, 4004 | 4010 | 4011 | 4012 | 4013 | 4014)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_names_as_secondbot_makes_them() {
        assert_eq!(channel_name("Test Group"), "test-group");
        assert_eq!(channel_name("someresident"), "someresident");
        assert_eq!(channel_name("A - B  & C!"), "a-b-c");
        assert_eq!(channel_name("Ünïcode Club"), "ncode-club");
    }

    #[test]
    fn long_messages_are_cut() {
        let long = "x".repeat(2500);
        assert_eq!(fit(&long).chars().count(), 1903);
        assert_eq!(fit("short"), "short");
    }
}
