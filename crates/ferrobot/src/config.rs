//! The bot's settings, from environment variables (the container's env).

use crate::commands::custom::{CustomCommand, Step};
use crate::services::interactions::{Level, Rule, Rules};
use ferro_client::login::{SL_MAIN_GRID, StartLocation};
use std::time::Duration;

#[derive(Clone)]
pub struct Config {
    pub login_uri: String,
    /// "First Last" or a single username.
    pub username: String,
    pub password: String,
    pub start: StartLocation,
    /// Log in again after the connection is lost.
    pub relog: bool,
    /// Seconds to wait before logging in again.
    pub relog_delay: u64,
    /// How long the client bears with a silent region or a failing event queue: the viewer's
    /// defaults, shorter only in tests (FERRO_REGION_TIMEOUT, FERRO_EVENT_QUEUE_ERRORS).
    pub timeouts: ferro_client::Timeouts,
    /// The avatars the bot obeys: "First Last" or usernames (SecondBot's MastersCSV).
    pub masters: Vec<String>,
    /// SecondBot's HTTP API: on unless turned off, and only with a shared secret.
    pub http_enabled: bool,
    pub http_port: u16,
    /// The address the API listens on: every interface by default, 127.0.0.1 behind a tunnel on
    /// the same host.
    pub http_bind: std::net::IpAddr,
    /// The secret that signs the API's commands. Never logged.
    pub shared_secret: Option<String>,
    /// How old a signed command may be, in seconds (SecondBot's TimeWindowSecs).
    pub time_window: u64,
    /// Commands by IM: from masters, and signed ones from objects (SecondBot's AllowIMcontrol).
    pub im_commands: bool,
    /// Commands by IM carry their time and are refused when too old (EnforceTimeWindow).
    pub enforce_time_window: bool,
    /// An object owned by a master needs no signature (ObjectMasterOptout).
    pub object_master_optout: bool,
    /// What the bot accepts from whom (SecondBot's InteractionConfig).
    pub interactions: Rules,
    /// Where the interactions are told as JSON events, when they are (EnableJsonOutputEvents,
    /// JsonOutputEventsTarget).
    pub interaction_events: Option<String>,
    /// Where script dialogs are relayed (SecondBot's DialogRelayConfig).
    pub dialog_relay: crate::dialogs::Targets,
    /// The relays (SecondBot's RelayConfig, and FERRO_RELAY_<n>).
    pub relays: Vec<crate::services::relays::Relay>,
    /// The Funds commands: off by default (SecondBot's AllowFundsCommands).
    pub allow_funds: bool,
    /// Commands that change things in world, per minute (0: no limit).
    pub rate_limit: u32,
    /// Homebound: off unless a home sim is given (SecondBot's HomeboundConfig).
    pub homebound: Option<crate::services::homebound::Settings>,
    /// Discord: off unless on, with a server and a token (SecondBot's DiscordConfig).
    pub discord: Option<crate::discord::Settings>,
    /// RabbitMQ: off unless on (SecondBot's RabbitConfig).
    pub rabbit: Option<crate::rabbit::Settings>,
    /// AI replies: off unless on (SecondBot's ChatGptConfig). Its key is never logged.
    pub chatgpt: Option<crate::services::chatgpt::Settings>,
    /// The operator's custom commands (SecondBot's CustomCommandsConfig).
    pub custom_commands: Vec<CustomCommand>,
    /// SecondBot's events service: off unless on (EventsConfig).
    pub events: Option<crate::services::events::Settings>,
    /// Commands run on events: off unless on, with events (SecondBot's OnEventConfig).
    pub on_event: Option<crate::services::on_event::Settings>,
    /// RLV: objects drive the bot with "@" commands; off by default (SecondBot's RlvConfig).
    pub rlv: bool,
    /// E-mail: off unless on, with a server (SecondBot's SmtpConfig).
    pub smtp: Option<crate::services::smtp::Settings>,
}

impl std::fmt::Debug for Config {
    // never prints the password or the shared secret
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("login_uri", &self.login_uri)
            .field("username", &self.username)
            .field("start", &self.start)
            .field("relog", &self.relog)
            .field("relog_delay", &self.relog_delay)
            .field("timeouts", &self.timeouts)
            .field("masters", &self.masters)
            .field("http_enabled", &self.http_enabled)
            .field("http_port", &self.http_port)
            .field("http_bind", &self.http_bind)
            .field("shared_secret_set", &self.shared_secret.is_some())
            .field("time_window", &self.time_window)
            .field("im_commands", &self.im_commands)
            .field("enforce_time_window", &self.enforce_time_window)
            .field("object_master_optout", &self.object_master_optout)
            .field("interactions", &self.interactions)
            .field("interaction_events", &self.interaction_events.is_some())
            .field("dialog_relay", &self.dialog_relay)
            .field("homebound", &self.homebound)
            .field("rate_limit", &self.rate_limit)
            .field("allow_funds", &self.allow_funds)
            .field("relays", &self.relays.len())
            .field("discord", &self.discord)
            .field("rabbit", &self.rabbit)
            .field("chatgpt", &self.chatgpt)
            // their steps may name reply targets: the triggers only
            .field(
                "custom_commands",
                &self
                    .custom_commands
                    .iter()
                    .map(|c| c.trigger.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("events", &self.events)
            .field(
                "on_event",
                &self.on_event.as_ref().map(|o| o.triggers.len()),
            )
            .field("rlv", &self.rlv)
            .field("smtp", &self.smtp)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0} is not set")]
    Missing(&'static str),
    #[error("{0}: {1}")]
    Invalid(&'static str, String),
}

/// A setting by its Ferrobot name, else by SecondBot's (`<file>_<Setting>`, as its Config class
/// reads the environment), so that a SecondBot container's settings work as they are. Returns the
/// name it was found under, for the error messages.
fn setting(ferro: &'static str, secondbot: &'static str) -> Option<(&'static str, String)> {
    let read = |name: &'static str| {
        std::env::var(name)
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
            .map(|v| (name, v))
    };
    read(ferro).or_else(|| read(secondbot))
}

fn var(name: &'static str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn parse_flag(name: &'static str, v: &str) -> Result<bool, ConfigError> {
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::Invalid(
            name,
            format!("\"{v}\" is not true or false"),
        )),
    }
}

fn setting_flag(
    ferro: &'static str,
    secondbot: &'static str,
    default: bool,
) -> Result<bool, ConfigError> {
    setting(ferro, secondbot).map_or(Ok(default), |(name, v)| parse_flag(name, &v))
}

/// Discord IDs, comma-separated (FERRO_DISCORD_MASTERS).
fn discord_ids(name: &'static str, v: &str) -> Result<Vec<u64>, ConfigError> {
    v.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse()
                .map_err(|_| ConfigError::Invalid(name, format!("\"{s}\" is not a Discord ID")))
        })
        .collect()
}

/// Discord (SecondBot's DiscordConfig): on with FERRO_DISCORD (discord_Enabled), a server
/// (FERRO_DISCORD_SERVER / discord_ServerID) and the bot's token (FERRO_DISCORD_TOKEN /
/// discord_ClientToken, a secret).
fn discord() -> Result<Option<crate::discord::Settings>, ConfigError> {
    if !setting_flag("FERRO_DISCORD", "discord_Enabled", false)? {
        return Ok(None);
    }
    let server = match setting("FERRO_DISCORD_SERVER", "discord_ServerID") {
        Some((name, v)) => v
            .parse::<u64>()
            .ok()
            .filter(|id| *id != 0)
            .ok_or(ConfigError::Invalid(name, "a Discord server ID".into()))?,
        None => return Err(ConfigError::Missing("FERRO_DISCORD_SERVER")),
    };
    // the token is taken as it is
    let token = ["FERRO_DISCORD_TOKEN", "discord_ClientToken"]
        .iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty())
        })
        .ok_or(ConfigError::Missing("FERRO_DISCORD_TOKEN"))?;
    let masters = match var("FERRO_DISCORD_MASTERS") {
        Some(v) => discord_ids("FERRO_DISCORD_MASTERS", &v)?,
        None => Vec::new(),
    };
    let interaction = if setting_flag(
        "FERRO_DISCORD_INTERACTION",
        "discord_InteractionEnabled",
        false,
    )? {
        let url = setting(
            "FERRO_DISCORD_INTERACTION_URL",
            "discord_InteractionHttpTarget",
        )
        .map(|(_, v)| v)
        .filter(|v| v.starts_with("http"));
        let channel = setting(
            "FERRO_DISCORD_INTERACTION_CHANNEL",
            "discord_InteractionChannelNumber",
        )
        .and_then(|(_, v)| v.parse::<u64>().ok());
        match (url, channel) {
            (Some(url), Some(channel)) => Some(crate::discord::Interaction {
                name: setting(
                    "FERRO_DISCORD_INTERACTION_NAME",
                    "discord_InteractionCommandName",
                )
                .map_or_else(|| "Go".to_owned(), |(_, v)| v),
                url,
                channel,
            }),
            _ => {
                log::warn!("Discord interaction off: it needs an http(s) address and a channel");
                None
            }
        }
    } else {
        None
    };
    Ok(Some(crate::discord::Settings {
        token,
        server,
        commands: setting_flag(
            "FERRO_DISCORD_COMMANDS",
            "discord_AllowDiscordCommands",
            false,
        )?,
        masters,
        hide_names: setting_flag("FERRO_DISCORD_HIDE_NAMES", "discord_hideChatterName", false)?,
        clear_on_connect: setting_flag(
            "FERRO_DISCORD_CLEAR_ON_CONNECT",
            "discord_ClearChatOnConnect",
            true,
        )?,
        interaction,
        stand_in: var("FERRO_DISCORD_STAND_IN"),
    }))
}

/// RabbitMQ (SecondBot's RabbitConfig): on with FERRO_RABBIT (rabbit_Enabled); the broker
/// FERRO_RABBIT_HOST (rabbit_HostIP, 127.0.0.1), FERRO_RABBIT_PORT (rabbit_HostPort, 5672, or
/// 5671 with TLS), FERRO_RABBIT_USERNAME (rabbit_HostUsername, guest), FERRO_RABBIT_PASSWORD
/// (rabbit_HostPassword, guest; a secret), FERRO_RABBIT_VHOST ("/") and FERRO_RABBIT_TLS (false);
/// the queues read FERRO_RABBIT_NOTECARD_QUEUE, _COMMAND_QUEUE, _IM_QUEUE, _GROUP_IM_QUEUE
/// (rabbit_NotecardQueue, rabbit_CommandQueue, rabbit_ImQueue, rabbit_GroupImQueue). SecondBot's
/// rabbit_LogDebug and rabbit_HideStatusOutput are not read: RUST_LOG sets what is logged.
fn rabbit() -> Result<Option<crate::rabbit::Settings>, ConfigError> {
    use crate::rabbit::{Queues, Settings, check_queue_name};
    if !setting_flag("FERRO_RABBIT", "rabbit_Enabled", false)? {
        return Ok(None);
    }
    let tls = flag("FERRO_RABBIT_TLS", false)?;
    let queue = |ferro: &'static str, secondbot: &'static str, default: String| match setting(
        ferro, secondbot,
    ) {
        None => Ok(default),
        Some((name, v)) => check_queue_name(&v)
            .map(|()| v)
            .map_err(|e| ConfigError::Invalid(name, e)),
    };
    let defaults = Queues::default();
    let vhost = var("FERRO_RABBIT_VHOST").unwrap_or_else(|| "/".to_owned());
    if vhost.len() > 255 {
        return Err(ConfigError::Invalid(
            "FERRO_RABBIT_VHOST",
            "at most 255 bytes".into(),
        ));
    }
    Ok(Some(Settings {
        host: setting("FERRO_RABBIT_HOST", "rabbit_HostIP")
            .map_or_else(|| "127.0.0.1".to_owned(), |(_, v)| v),
        port: number(
            setting("FERRO_RABBIT_PORT", "rabbit_HostPort"),
            if tls { 5671 } else { 5672 },
            "a port number",
        )?,
        username: setting("FERRO_RABBIT_USERNAME", "rabbit_HostUsername")
            .map_or_else(|| "guest".to_owned(), |(_, v)| v),
        // the password is taken as it is, spaces included
        password: ["FERRO_RABBIT_PASSWORD", "rabbit_HostPassword"]
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|p| !p.is_empty()))
            .unwrap_or_else(|| "guest".to_owned()),
        vhost,
        tls,
        queues: Queues {
            notecards: queue(
                "FERRO_RABBIT_NOTECARD_QUEUE",
                "rabbit_NotecardQueue",
                defaults.notecards,
            )?,
            commands: queue(
                "FERRO_RABBIT_COMMAND_QUEUE",
                "rabbit_CommandQueue",
                defaults.commands,
            )?,
            ims: queue("FERRO_RABBIT_IM_QUEUE", "rabbit_ImQueue", defaults.ims)?,
            group_ims: queue(
                "FERRO_RABBIT_GROUP_IM_QUEUE",
                "rabbit_GroupImQueue",
                defaults.group_ims,
            )?,
        },
    }))
}

/// AI replies (SecondBot's ChatGptConfig, file "chatgpt"): on with FERRO_CHATGPT
/// (chatgpt_Enabled). The key (FERRO_CHATGPT_API_KEY / chatgpt_ApiKey) is a secret, taken as it
/// is; "none", SecondBot's default, is no key. A cooldown and the history size are held in
/// SecondBot's ranges (1 to 10 s, 3 to 10 lines).
fn chatgpt() -> Result<Option<crate::services::chatgpt::Settings>, ConfigError> {
    use crate::services::chatgpt::{self as ai, in_range};
    if !setting_flag("FERRO_CHATGPT", "chatgpt_Enabled", false)? {
        return Ok(None);
    }
    let none = |v: String| (!v.eq_ignore_ascii_case("none")).then_some(v);
    let api_key = ["FERRO_CHATGPT_API_KEY", "chatgpt_ApiKey"]
        .iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .map(|k| k.trim().to_owned())
                .filter(|k| !k.is_empty())
        })
        .and_then(none);
    let provider = setting("FERRO_CHATGPT_PROVIDER", "chatgpt_Provider");
    let (provider_name, provider) =
        provider.unwrap_or(("FERRO_CHATGPT_PROVIDER", ai::OPENAI.to_owned()));
    let endpoint = ai::endpoint(&provider).map_err(|e| ConfigError::Invalid(provider_name, e))?;
    if api_key.is_none() && provider.eq_ignore_ascii_case(ai::OPENAI) {
        return Err(ConfigError::Missing("FERRO_CHATGPT_API_KEY"));
    }
    let seconds = |found, what| -> Result<u64, ConfigError> {
        let n: i64 = number(found, 3, what)?;
        Ok(u64::try_from(in_range(n, 1, ai::MAX_COOLDOWN as i64)).unwrap_or(1))
    };
    let history: i64 = number(
        setting("FERRO_CHATGPT_HISTORY", "chatgpt_ChatHistoryMessages"),
        5,
        "a number of lines",
    )?;
    let timeout: u64 = number(
        setting(
            "FERRO_CHATGPT_HISTORY_TIMEOUT",
            "chatgpt_ChatHistoryTimeout",
        ),
        15,
        "a number of minutes",
    )?;
    let group = if setting_flag("FERRO_CHATGPT_GROUP", "chatgpt_AllowGroupReplys", false)? {
        // SecondBot's default group is "none": no group answered
        match setting("FERRO_CHATGPT_GROUP_KEY", "chatgpt_GroupReplyForGroup")
            .filter(|(_, v)| !v.eq_ignore_ascii_case("none"))
        {
            Some((name, v)) => Some(
                uuid::Uuid::parse_str(&v)
                    .map_err(|_| ConfigError::Invalid(name, "a group's key".into()))?,
            ),
            None => {
                log::warn!("AI replies to group chat off: no group (FERRO_CHATGPT_GROUP_KEY)");
                None
            }
        }
    } else {
        None
    };
    if flag("chatgpt_UseRedis", false)? {
        log::warn!("chatgpt_UseRedis is ignored: the conversations are kept in memory");
    }
    let settings = ai::Settings {
        api_key,
        organization: setting("FERRO_CHATGPT_ORGANIZATION", "chatgpt_OrganizationId")
            .map(|(_, v)| v)
            .and_then(none),
        endpoint,
        model: setting("FERRO_CHATGPT_MODEL", "chatgpt_UseModel")
            .map_or_else(|| ai::DEFAULT_MODEL.to_owned(), |(_, v)| v),
        prompt: setting("FERRO_CHATGPT_PROMPT", "chatgpt_ChatPrompt")
            .map_or_else(|| ai::DEFAULT_PROMPT.to_owned(), |(_, v)| v),
        name: setting("FERRO_CHATGPT_NAME", "chatgpt_CustomName")
            .map(|(_, v)| v)
            .filter(|v| v != "<!FIRSTNAME!>"),
        history: usize::try_from(in_range(history, 3, 10)).unwrap_or(5),
        history_timeout: std::time::Duration::from_secs(timeout.saturating_mul(60)),
        localchat: setting_flag("FERRO_CHATGPT_LOCALCHAT", "chatgpt_LocalchatReply", false)?,
        localchat_cooldown: seconds(
            setting(
                "FERRO_CHATGPT_LOCALCHAT_COOLDOWN",
                "chatgpt_LocalchatRateLimiter",
            ),
            "a number of seconds",
        )?,
        im: setting_flag("FERRO_CHATGPT_IM", "chatgpt_AllowImReplys", false)?,
        im_friends_only: setting_flag(
            "FERRO_CHATGPT_IM_FRIENDS_ONLY",
            "chatgpt_ImReplyFriendsOnly",
            false,
        )?,
        im_cooldown: seconds(
            setting("FERRO_CHATGPT_IM_COOLDOWN", "chatgpt_ImReplyRateLimiter"),
            "a number of seconds",
        )?,
        group,
        group_cooldown: seconds(
            setting(
                "FERRO_CHATGPT_GROUP_COOLDOWN",
                "chatgpt_GroupReplyRateLimiter",
            ),
            "a number of seconds",
        )?,
        debug: setting_flag("FERRO_CHATGPT_DEBUG", "chatgpt_ShowDebug", false)?,
        typing: setting_flag("FERRO_CHATGPT_TYPING", "chatgpt_FakeTypeDelay", true)?,
        provider,
    };
    if !settings.localchat && !settings.im && settings.group.is_none() {
        log::warn!(
            "AI replies on, but to nothing: turn on FERRO_CHATGPT_LOCALCHAT, FERRO_CHATGPT_IM or FERRO_CHATGPT_GROUP"
        );
    }
    Ok(Some(settings))
}

/// E-mail (SecondBot's SmtpConfig): on with FERRO_SMTP (smtp_Enabled) and a server
/// (FERRO_SMTP_HOST / smtp_Host). The password (FERRO_SMTP_PASSWORD / smtp_Token, "none" for no
/// login) is a secret; the addresses are personal data: neither is logged.
fn smtp() -> Result<Option<crate::services::smtp::Settings>, ConfigError> {
    smtp_from(&|name| std::env::var(name).ok())
}

/// The same, from any variables (the tests' own).
fn smtp_from(
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<crate::services::smtp::Settings>, ConfigError> {
    use crate::services::smtp::{Security, Settings};
    // by Ferrobot's name, else by SecondBot's, as `setting`
    let get = |names: &[&'static str]| {
        names.iter().find_map(|&name| {
            env(name)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
                .map(|v| (name, v))
        })
    };
    let flag = |names: &[&'static str], default: bool| {
        get(names).map_or(Ok(default), |(name, v)| parse_flag(name, &v))
    };
    if !flag(&["FERRO_SMTP", "smtp_Enabled"], false)? {
        return Ok(None);
    }
    let host = get(&["FERRO_SMTP_HOST", "smtp_Host"])
        .map(|(_, v)| v)
        .ok_or(ConfigError::Missing("FERRO_SMTP_HOST"))?;
    let security = match get(&["FERRO_SMTP_TLS"]) {
        Some((name, v)) => {
            Security::parse(&v).ok_or(ConfigError::Invalid(name, "tls, starttls or none".into()))?
        }
        None if flag(&["smtp_useSSL"], false)? => Security::Tls,
        None => Security::StartTls,
    };
    // the password is taken as it is, spaces included; SecondBot's "none" is no login
    let password = ["FERRO_SMTP_PASSWORD", "smtp_Token"]
        .iter()
        .find_map(|name| env(name).filter(|p| !p.is_empty()))
        .filter(|p| p != "none");
    let login = match password {
        None => None,
        Some(password) => Some((
            get(&["FERRO_SMTP_USER", "smtp_User"])
                .map(|(_, v)| v)
                .ok_or(ConfigError::Missing("FERRO_SMTP_USER"))?,
            password,
        )),
    };
    // SecondBot's placeholders mean "not set"
    let address = |names: &[&'static str], placeholder: &str| match get(names) {
        Some((_, v)) if v.eq_ignore_ascii_case(placeholder) => Ok(None),
        Some((name, v)) => v
            .parse::<lettre::Address>()
            .map(Some)
            .map_err(|_| ConfigError::Invalid(name, "an e-mail address".into())),
        None => Ok(None),
    };
    let allowed = match get(&["FERRO_SMTP_ALLOWED"]) {
        Some((_, csv)) => Some(csv),
        None if flag(&["smtp_UseAllowedRecivers"], false)? => Some(
            get(&["smtp_AllowedReciversCSV"])
                .map(|(_, v)| v)
                .unwrap_or_default(),
        ),
        None => None,
    }
    .map(|csv| {
        csv.split(',')
            .map(str::trim)
            .filter(|a| !a.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    });
    let settings = Settings {
        host,
        port: number(get(&["FERRO_SMTP_PORT", "smtp_Port"]), 587, "a port number")?,
        security,
        login,
        from: address(
            &["FERRO_SMTP_FROM", "smtp_MailReplyAddress"],
            "me@myemail.tld",
        )?,
        command: flag(&["FERRO_SMTP_COMMAND", "smtp_AllowCommandSendMail"], false)?,
        reply_target: flag(
            &["FERRO_SMTP_REPLY_TARGET", "smtp_AllowEmailAsReplyTarget"],
            false,
        )?,
        allowed,
        alerts: flag(&["FERRO_SMTP_ALERTS", "smtp_AllowSendAlertStatus"], false)?,
        login_notice: flag(
            &["FERRO_SMTP_LOGIN_NOTICE", "smtp_AllowSendLoginNotice"],
            false,
        )?,
        notify: address(
            &["FERRO_SMTP_NOTIFY", "smtp_SendAlertsAndLoginsTo"],
            "me@somewhere.tld",
        )?,
    };
    if settings.from.is_none() {
        log::warn!("e-mail: no reply address (FERRO_SMTP_FROM): nothing can be sent");
    }
    if settings.security == Security::None && settings.login.is_some() {
        log::warn!("e-mail: FERRO_SMTP_TLS is none: the SMTP password goes out unencrypted");
    }
    if (settings.command || settings.reply_target) && settings.allowed.is_none() {
        log::warn!(
            "e-mail: commands may mail any address; FERRO_SMTP_ALLOWED limits the recipients"
        );
    }
    Ok(Some(settings))
}

/// The relays: FERRO_RELAY_1, FERRO_RELAY_2… in SecondBot's short form, then SecondBot's own
/// (relay_count, and for each relay_<n>_enabled, _extended, _source_type, _source_option,
/// _target_type, _target_option, or relay_<n>_RelayConfig with relay_UseShortEncoder).
fn relays() -> Result<Vec<crate::services::relays::Relay>, ConfigError> {
    use crate::services::relays::Relay;
    let mut out = Vec::new();
    for n in 1.. {
        let Ok(v) = std::env::var(format!("FERRO_RELAY_{n}")) else {
            break;
        };
        out.push(Relay::parse_short(&v).ok_or(ConfigError::Invalid(
            "FERRO_RELAY_<n>",
            "sType::…{}sFilter::…{}tType::…{}tConfig::…".into(),
        ))?);
    }
    let count: u32 = number(
        var("relay_count").map(|v| ("relay_count", v)),
        0,
        "a number",
    )?;
    let short = flag("relay_UseShortEncoder", false)?;
    let get = |n: u32, k: &str| {
        std::env::var(format!("relay_{n}_{k}"))
            .ok()
            .map(|v| v.trim().to_owned())
    };
    for n in 1..=count {
        if short {
            if let Some(r) = get(n, "RelayConfig").and_then(|v| Relay::parse_short(&v)) {
                out.push(r);
            }
            continue;
        }
        let on = |k: &str| get(n, k).is_some_and(|v| v.eq_ignore_ascii_case("true"));
        if !on("enabled") {
            continue;
        }
        out.push(Relay {
            source_type: get(n, "source_type").unwrap_or_default(),
            source_option: get(n, "source_option").unwrap_or_default(),
            target_type: get(n, "target_type").unwrap_or_default(),
            target_option: get(n, "target_option").unwrap_or_default(),
            extended: on("extended"),
        });
    }
    Ok(out)
}

/// One custom command's settings, read under one naming.
struct CustomNames {
    /// What names its settings in the error messages.
    pattern: &'static str,
    trigger: String,
    args: String,
    step: Box<dyn Fn(usize) -> String>,
}

/// The custom commands: FERRO_CUSTOM_COMMAND_1_TRIGGER, _ARGS and _STEP_1, _STEP_2… for each,
/// counted until a trigger is missing; then SecondBot's (customcommands_count, and for each
/// customcommands_<n>_trigger, _args, _steps and _commands_<m>). SecondBot's count is 1 by
/// default, with an "exitplease" that runs nothing; here there is none unless set. A missing step
/// or trigger, a trigger given twice or one that is a built-in command's name, stop the bot
/// (SecondBot fills a missing step with "Logoff").
fn custom_commands() -> Result<Vec<CustomCommand>, ConfigError> {
    custom_commands_from(|name| std::env::var(name).ok())
}

/// The custom commands, from the settings `get` gives.
fn custom_commands_from(
    get: impl Fn(&str) -> Option<String>,
) -> Result<Vec<CustomCommand>, ConfigError> {
    let raw = |name: &str| get(name).filter(|v| !v.trim().is_empty());
    let mut named = Vec::new();
    for n in 1.. {
        let trigger = format!("FERRO_CUSTOM_COMMAND_{n}_TRIGGER");
        if raw(&trigger).is_none() {
            break;
        }
        named.push((
            None,
            CustomNames {
                pattern: "FERRO_CUSTOM_COMMAND_<n>_…",
                trigger,
                args: format!("FERRO_CUSTOM_COMMAND_{n}_ARGS"),
                step: Box::new(move |m| format!("FERRO_CUSTOM_COMMAND_{n}_STEP_{m}")),
            },
        ));
    }
    let count: usize = number(
        raw("customcommands_count").map(|v| ("customcommands_count", v.trim().to_owned())),
        0,
        "a number of custom commands",
    )?;
    for n in 1..=count {
        let steps: usize = number(
            raw(&format!("customcommands_{n}_steps"))
                .map(|v| ("customcommands_<n>_steps", v.trim().to_owned())),
            0,
            "a number of steps",
        )?;
        named.push((
            Some(steps),
            CustomNames {
                pattern: "customcommands_<n>_…",
                trigger: format!("customcommands_{n}_trigger"),
                args: format!("customcommands_{n}_args"),
                step: Box::new(move |m| format!("customcommands_{n}_commands_{m}")),
            },
        ));
    }
    let mut out: Vec<CustomCommand> = Vec::new();
    for (steps, names) in named {
        let invalid = |what: String| ConfigError::Invalid(names.pattern, what);
        let trigger = raw(&names.trigger)
            .map(|t| t.trim().to_owned())
            .ok_or_else(|| invalid(format!("{} is not set", names.trigger)))?;
        if let Some(builtin) = crate::commands::known(&trigger) {
            return Err(invalid(format!(
                "{}: \"{trigger}\" is the built-in command {builtin}",
                names.trigger
            )));
        }
        if out.iter().any(|c| c.trigger == trigger) {
            return Err(invalid(format!(
                "{}: \"{trigger}\" is given twice",
                names.trigger
            )));
        }
        let args = match raw(&names.args) {
            None => 0,
            Some(v) => v
                .trim()
                .parse()
                .map_err(|_| invalid(format!("{}: a number of arguments", names.args)))?,
        };
        let mut list = Vec::new();
        for m in 1.. {
            if steps.is_some_and(|s| m > s) {
                break;
            }
            let name = (names.step)(m);
            let Some(text) = raw(&name) else {
                if steps.is_some() {
                    return Err(invalid(format!("{name} is not set")));
                }
                break;
            };
            let step =
                Step::parse(&text).ok_or_else(|| invalid(format!("{name}: names no command")))?;
            if let Some(k) = step.placeholders().find(|k| *k > args) {
                log::warn!(
                    "custom command {trigger}: {name} has [C_ARG_{k}] but the command takes \
                     {args} arguments: it stays as it is"
                );
            }
            list.push(step);
        }
        if list.is_empty() {
            return Err(invalid(format!(
                "{}: \"{trigger}\" has no steps",
                names.trigger
            )));
        }
        out.push(CustomCommand {
            trigger,
            args,
            steps: list,
        });
    }
    Ok(out)
}

/// Homebound's settings: on with a home sim (FERRO_HOME_SIM, or SecondBot's homebound_Enabled
/// with homebound_HomeSimSlUrl).
fn homebound() -> Result<Option<crate::services::homebound::Settings>, ConfigError> {
    use crate::services::homebound::{Place, Settings};
    let home = match var("FERRO_HOME_SIM") {
        Some(v) => Some(("FERRO_HOME_SIM", v)),
        None if setting_flag("FERRO_HOMEBOUND", "homebound_Enabled", false)? => {
            setting("FERRO_HOME_SIM", "homebound_HomeSimSlUrl")
        }
        None => None,
    };
    let Some((name, home)) = home else {
        return Ok(None);
    };
    let place = |name: &'static str, v: &str| {
        Place::parse(v).ok_or(ConfigError::Invalid(name, "Region Name/x/y/z".into()))
    };
    let backup = match setting("FERRO_BACKUP_SIM", "homebound_BackupSimSLUrl") {
        Some((name, v)) => Some(place(name, &v)?),
        None => None,
    };
    let minutes: u64 = number(
        setting("FERRO_HOME_AFTER", "homebound_ReturnToHomeSimAfterMins"),
        1,
        "a number of minutes",
    )?;
    Ok(Some(Settings {
        home: place(name, &home)?,
        backup,
        after: std::time::Duration::from_secs(minutes * 60),
        seek: setting_flag("FERRO_HOME_SEEK", "homebound_AtHomeSeekLocation", true)?,
        // SecondBot's default is the nil key: no seat
        sit: match setting("FERRO_HOME_SIT", "homebound_AtHomeAutoSitUuid") {
            None => None,
            Some((name, v)) => Some(
                uuid::Uuid::parse_str(&v)
                    .map_err(|_| ConfigError::Invalid(name, "an object's key".into()))?,
            )
            .filter(|id| !id.is_nil()),
        },
    }))
}

/// Seconds, at least 1.
fn seconds(found: Option<(&'static str, String)>, default: u64) -> Result<Duration, ConfigError> {
    let n: u64 = number(found, default, "a number of seconds, at least 1")?;
    if n == 0 {
        return Err(ConfigError::Invalid(
            "a check's pace",
            "a number of seconds, at least 1".into(),
        ));
    }
    Ok(Duration::from_secs(n))
}

/// SecondBot's events service (EventsConfig): on with FERRO_EVENTS (events_Enabled), each event
/// on unless turned off, as SecondBot's defaults.
fn events() -> Result<Option<crate::services::events::Settings>, ConfigError> {
    if !setting_flag("FERRO_EVENTS", "events_Enabled", false)? {
        return Ok(None);
    }
    let key = |ferro: &'static str, secondbot: &'static str| {
        setting(ferro, secondbot)
            .and_then(|(_, v)| uuid::Uuid::parse_str(&v).ok())
            .filter(|id| !id.is_nil())
    };
    let channel: i32 = number(
        setting("FERRO_EVENTS_CHANNEL", "events_OutputChannel"),
        -1,
        "a channel number",
    )?;
    // SecondBot's defaults: "example" names no place, "n/a" no address
    let name = |ferro: &'static str, secondbot: &'static str| {
        setting(ferro, secondbot).map_or_else(|| "example".to_owned(), |(_, v)| v)
    };
    let secret = ["FERRO_EVENTS_SECRET", "events_OutputSecret"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|s| !s.is_empty()));
    if secret.is_none() {
        log::warn!("events: no FERRO_EVENTS_SECRET, SecondBot's default one signs them");
    }
    Ok(Some(crate::services::events::Settings {
        group: key("FERRO_EVENTS_GROUP", "events_GroupMemberEventsGroupUUID"),
        group_joins: setting_flag("FERRO_EVENTS_GROUP_JOINS", "events_GroupMemberJoins", true)?,
        group_leaves: setting_flag(
            "FERRO_EVENTS_GROUP_LEAVES",
            "events_GroupMemberLeaves",
            true,
        )?,
        group_every: seconds(
            var("FERRO_EVENTS_GROUP_CHECK").map(|v| ("FERRO_EVENTS_GROUP_CHECK", v)),
            120,
        )?,
        guest_enters: setting_flag("FERRO_EVENTS_GUEST_ENTERS", "events_GuestEntersArea", true)?,
        guest_leaves: setting_flag("FERRO_EVENTS_GUEST_LEAVES", "events_GuestLeavesArea", true)?,
        guest_sim: name("FERRO_EVENTS_GUEST_SIM", "events_GuestTrackingSimname"),
        guest_parcel: name(
            "FERRO_EVENTS_GUEST_PARCEL",
            "events_GuestTrackingParcelname",
        ),
        guest_every: seconds(
            var("FERRO_EVENTS_GUEST_CHECK").map(|v| ("FERRO_EVENTS_GUEST_CHECK", v)),
            30,
        )?,
        alerts: setting_flag("FERRO_EVENTS_ALERTS", "events_SimAlertMessage", true)?,
        status: setting_flag("FERRO_EVENTS_STATUS", "events_StatusMessage", true)?,
        money: setting_flag("FERRO_EVENTS_MONEY", "events_MoneyEvent", true)?,
        change_sim: setting_flag("FERRO_EVENTS_CHANGE_SIM", "events_ChangeSim", true)?,
        change_parcel: setting_flag("FERRO_EVENTS_CHANGE_PARCEL", "events_ChangeParcel", true)?,
        channel: (channel >= 0).then_some(channel),
        avatar: key("FERRO_EVENTS_AVATAR", "events_OutputIMuuid"),
        url: setting("FERRO_EVENTS_HTTP", "events_OutputHttpURL")
            .map(|(_, v)| v)
            .filter(|v| v.starts_with("http")),
        secret: secret.unwrap_or_else(|| "donttellanyone".to_owned()),
    }))
}

/// The events of the commands run on events: FERRO_ON_EVENT_<n>_SOURCE, _MONITOR, _WHERE_<m>,
/// _ACTION_<m> (and _ENABLED, true by default), then SecondBot's (onevent_Count, and for each
/// onevent_Event<n>Enabled, Source, Monitor, WhereCount, Where<m>, ActionCount, Action<m>). An
/// event that does not hold together is left out and logged, as SecondBot does.
fn on_event_triggers() -> Result<Vec<crate::on_event::Trigger>, ConfigError> {
    use crate::on_event::{Raw, Trigger};
    let get = |name: String| std::env::var(name).ok().map(|v| v.trim().to_owned());
    // until the first one missing
    let list = |prefix: &str| -> Vec<String> {
        (1..).map_while(|m| get(format!("{prefix}{m}"))).collect()
    };
    let mut raws: Vec<(String, Raw)> = Vec::new();
    for n in 1.. {
        let Some(source) = get(format!("FERRO_ON_EVENT_{n}_SOURCE")) else {
            break;
        };
        let p = format!("FERRO_ON_EVENT_{n}_");
        let enabled = match get(format!("{p}ENABLED")) {
            None => true,
            Some(v) => parse_flag("FERRO_ON_EVENT_<n>_ENABLED", &v)?,
        };
        if !enabled {
            log::info!("on event FERRO_ON_EVENT_{n}: not enabled");
            continue;
        }
        raws.push((
            format!("FERRO_ON_EVENT_{n}"),
            Raw {
                source,
                monitor: get(format!("{p}MONITOR")).unwrap_or_default(),
                wheres: list(&format!("{p}WHERE_")),
                actions: list(&format!("{p}ACTION_")),
            },
        ));
    }
    let count = |name: String| get(name).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    for n in 1..=count("onevent_Count".into()) {
        let p = format!("onevent_Event{n}");
        // bool.TryParse, false when it is not one
        if get(format!("{p}Enabled")).and_then(|v| crate::commands::cs_bool(&v)) != Some(true) {
            log::info!("on event {n}: not enabled");
            continue;
        }
        let numbered = |what: &str| -> Vec<String> {
            (1..=count(format!("{p}{what}Count")))
                .map(|m| get(format!("{p}{what}{m}")).unwrap_or_default())
                .collect()
        };
        raws.push((
            format!("{n}"),
            Raw {
                source: get(format!("{p}Source")).unwrap_or_default(),
                monitor: get(format!("{p}Monitor")).unwrap_or_default(),
                wheres: numbered("Where"),
                actions: numbered("Action"),
            },
        ));
    }
    let mut out = Vec::new();
    for (i, (name, raw)) in raws.iter().enumerate() {
        match Trigger::parse(i + 1, raw) {
            Ok(t) => {
                log::info!("on event {name}: event {} here", t.n);
                out.push(t);
            }
            Err(why) => log::warn!("on event {name} left out: {why}"),
        }
    }
    Ok(out)
}

/// Commands run on events (SecondBot's OnEventConfig): on with FERRO_ON_EVENT (onevent_Enabled)
/// and at least one event that holds together.
fn on_event() -> Result<Option<crate::services::on_event::Settings>, ConfigError> {
    if !setting_flag("FERRO_ON_EVENT", "onevent_Enabled", false)? {
        return Ok(None);
    }
    let triggers = on_event_triggers()?;
    if triggers.is_empty() {
        log::warn!("on event: no events, off");
        return Ok(None);
    }
    let wait: u64 = number(
        setting("FERRO_ON_EVENT_WAIT", "onevent_WaitSecsToStart"),
        10,
        "a number of seconds",
    )?;
    Ok(Some(crate::services::on_event::Settings {
        triggers,
        wait: Duration::from_secs(wait),
        debug: setting_flag("FERRO_ON_EVENT_DEBUG", "onevent_DebugMe", false)?,
        guest_every: seconds(
            var("FERRO_ON_EVENT_GUEST_CHECK").map(|v| ("FERRO_ON_EVENT_GUEST_CHECK", v)),
            10,
        )?,
        group_every: seconds(
            var("FERRO_ON_EVENT_GROUP_CHECK").map(|v| ("FERRO_ON_EVENT_GROUP_CHECK", v)),
            15 * 60,
        )?,
    }))
}

/// The interaction events' target: off by default, as SecondBot's EnableJsonOutputEvents, and
/// off when on without a target ("none", SecondBot's default target, sends nowhere).
fn interaction_events() -> Result<Option<String>, ConfigError> {
    if !setting_flag(
        "FERRO_INTERACTION_EVENTS",
        "interaction_EnableJsonOutputEvents",
        false,
    )? {
        return Ok(None);
    }
    let target = setting(
        "FERRO_INTERACTION_EVENTS_TARGET",
        "interaction_JsonOutputEventsTarget",
    )
    .map(|(_, v)| v)
    .filter(|v| !v.eq_ignore_ascii_case("none"));
    if target.is_none() {
        log::warn!("interaction events off: they need a target (FERRO_INTERACTION_EVENTS_TARGET)");
    }
    Ok(target)
}

/// An interaction rule: its switch (on by default) and its level ("Owner" by default).
fn rule(
    switch: (&'static str, &'static str),
    level: (&'static str, &'static str),
) -> Result<Rule, ConfigError> {
    Ok(Rule {
        enabled: setting_flag(switch.0, switch.1, true)?,
        level: match setting(level.0, level.1) {
            None => Level::Owner,
            Some((name, v)) => Level::parse(&v).ok_or(ConfigError::Invalid(
                name,
                "Owner, Friends or Anyone".into(),
            ))?,
        },
    })
}

/// FERRO_REGION_TIMEOUT (seconds of silence, 100) and FERRO_EVENT_QUEUE_ERRORS (fast failures in
/// a row, 15): the viewer's limits, which the tests shorten so that they need not wait minutes.
fn timeouts() -> Result<ferro_client::Timeouts, ConfigError> {
    let default = ferro_client::Timeouts::default();
    let silence: u64 = number(
        var("FERRO_REGION_TIMEOUT").map(|v| ("FERRO_REGION_TIMEOUT", v)),
        default.region_silence.as_secs(),
        "a number of seconds, at least 1",
    )?;
    if silence == 0 {
        return Err(ConfigError::Invalid(
            "FERRO_REGION_TIMEOUT",
            "a number of seconds, at least 1".into(),
        ));
    }
    Ok(ferro_client::Timeouts {
        region_silence: std::time::Duration::from_secs(silence),
        event_queue_errors: number(
            var("FERRO_EVENT_QUEUE_ERRORS").map(|v| ("FERRO_EVENT_QUEUE_ERRORS", v)),
            default.event_queue_errors,
            "a number of errors",
        )?,
    })
}

fn flag(name: &'static str, default: bool) -> Result<bool, ConfigError> {
    var(name).map_or(Ok(default), |v| parse_flag(name, &v))
}

fn number<T: std::str::FromStr>(
    found: Option<(&'static str, String)>,
    default: T,
    what: &str,
) -> Result<T, ConfigError> {
    match found {
        None => Ok(default),
        Some((name, v)) => v
            .parse()
            .map_err(|_| ConfigError::Invalid(name, what.to_owned())),
    }
}

/// SecondBot's LoginURI: "secondlife" for the main grid, else an address.
fn login_uri(v: &str) -> String {
    if v.eq_ignore_ascii_case("secondlife") {
        SL_MAIN_GRID.to_owned()
    } else {
        v.to_owned()
    }
}

impl Config {
    /// FERRO_USERNAME, FERRO_PASSWORD (required); FERRO_LOGIN_URI (Second Life by default),
    /// FERRO_START ("last", "home" or "Region Name/x/y/z"), FERRO_RELOG (true), FERRO_RELOG_DELAY
    /// (60 s), FERRO_MASTERS (none), FERRO_HTTP (true), FERRO_HTTP_PORT (80), FERRO_SHARED_SECRET
    /// (none: the API stays off), FERRO_TIME_WINDOW (35 s). Each also under SecondBot's name
    /// (basic_Username, basic_Password, basic_LoginURI, commands_MastersCSV, http_Enabled,
    /// http_Port, commands_SharedSecret, commands_TimeWindowSecs).
    pub fn from_env() -> Result<Config, ConfigError> {
        let start = match var("FERRO_START") {
            None => StartLocation::Last,
            Some(s) => parse_start(&s).ok_or(ConfigError::Invalid(
                "FERRO_START",
                "use last, home or Region Name/x/y/z".into(),
            ))?,
        };
        let relog_delay = number(
            setting("FERRO_RELOG_DELAY", "FERRO_RELOG_DELAY"),
            60,
            "a number of seconds",
        )?;
        let timeouts = timeouts()?;
        // the password is taken as it is, spaces included
        let password = ["FERRO_PASSWORD", "basic_Password"]
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|p| !p.is_empty()))
            .ok_or(ConfigError::Missing("FERRO_PASSWORD"))?;
        let http_enabled = match setting("FERRO_HTTP", "http_Enabled") {
            None => true,
            Some((name, v)) => parse_flag(name, &v)?,
        };
        if setting("FERRO_HTTP_NO_VALIDATION", "http_DisableCommandValidation")
            .is_some_and(|(_, v)| v.eq_ignore_ascii_case("true"))
        {
            // Every command from outside is authenticated (AGENTS.md section 5): not followed.
            log::warn!("http_DisableCommandValidation is ignored: commands are always signed");
        }
        Ok(Config {
            login_uri: setting("FERRO_LOGIN_URI", "basic_LoginURI")
                .map_or_else(|| SL_MAIN_GRID.to_owned(), |(_, v)| login_uri(&v)),
            username: setting("FERRO_USERNAME", "basic_Username")
                .map(|(_, v)| v)
                .ok_or(ConfigError::Missing("FERRO_USERNAME"))?,
            password,
            start,
            relog: flag("FERRO_RELOG", true)?,
            relog_delay,
            timeouts,
            masters: setting("FERRO_MASTERS", "commands_MastersCSV")
                .map(|(_, v)| parse_masters(&v))
                .unwrap_or_default(),
            http_enabled,
            http_port: number(setting("FERRO_HTTP_PORT", "http_Port"), 80, "a port number")?,
            http_bind: number(
                var("FERRO_HTTP_BIND").map(|v| ("FERRO_HTTP_BIND", v)),
                std::net::IpAddr::from([0, 0, 0, 0]),
                "an IP address, such as 127.0.0.1",
            )?,
            shared_secret: ["FERRO_SHARED_SECRET", "commands_SharedSecret"]
                .iter()
                .find_map(|name| std::env::var(name).ok().filter(|s| !s.is_empty())),
            time_window: number(
                setting("FERRO_TIME_WINDOW", "commands_TimeWindowSecs"),
                35,
                "a number of seconds",
            )?,
            im_commands: setting_flag("FERRO_IM_COMMANDS", "commands_AllowIMcontrol", true)?,
            enforce_time_window: setting_flag(
                "FERRO_ENFORCE_TIME_WINDOW",
                "commands_EnforceTimeWindow",
                false,
            )?,
            object_master_optout: setting_flag(
                "FERRO_OBJECT_MASTER_OPTOUT",
                "commands_ObjectMasterOptout",
                false,
            )?,
            homebound: homebound()?,
            discord: discord()?,
            rabbit: rabbit()?,
            chatgpt: chatgpt()?,
            events: events()?,
            on_event: on_event()?,
            smtp: smtp()?,
            relays: relays()?,
            custom_commands: custom_commands()?,
            allow_funds: setting_flag("FERRO_ALLOW_FUNDS", "commands_AllowFundsCommands", false)?,
            rlv: setting_flag("FERRO_RLV", "rlv_Enabled", false)?,
            rate_limit: number(
                var("FERRO_RATE_LIMIT").map(|v| ("FERRO_RATE_LIMIT", v)),
                60,
                "a number of commands per minute",
            )?,
            dialog_relay: crate::dialogs::Targets {
                channel: number(
                    setting("FERRO_DIALOG_CHANNEL", "dialogrelay_RelayChannel"),
                    -1,
                    "a channel number",
                )
                .map(|c: i32| (c >= 0).then_some(c))?,
                avatar: setting("FERRO_DIALOG_AVATAR", "dialogrelay_RelayAvatar")
                    .and_then(|(_, v)| uuid::Uuid::parse_str(&v).ok()),
                url: setting("FERRO_DIALOG_HTTP", "dialogrelay_RelayHttpurl")
                    .map(|(_, v)| v)
                    .filter(|v| v.starts_with("http")),
                owner_only: setting("FERRO_DIALOG_OWNER", "dialogrelay_RelayObjectOwnerOnly")
                    .and_then(|(_, v)| uuid::Uuid::parse_str(&v).ok()),
            },
            interaction_events: interaction_events()?,
            interactions: Rules {
                teleports: rule(
                    ("FERRO_ACCEPT_TELEPORTS", "interaction_AcceptTeleports"),
                    ("FERRO_TELEPORT_LEVEL", "interaction_TeleportRequestLevel"),
                )?,
                group_invites: rule(
                    (
                        "FERRO_ACCEPT_GROUP_INVITES",
                        "interaction_AcceptGroupInvites",
                    ),
                    ("FERRO_GROUP_INVITE_LEVEL", "interaction_GroupInviteLevel"),
                )?,
                inventory: rule(
                    ("FERRO_ACCEPT_INVENTORY", "interaction_AcceptInventory"),
                    (
                        "FERRO_INVENTORY_LEVEL",
                        "interaction_InventoryTransferLevel",
                    ),
                )?,
                friendships: rule(
                    ("FERRO_ACCEPT_FRIENDS", "interaction_AcceptFriendRequests"),
                    ("FERRO_FRIEND_LEVEL", "interaction_FriendRequestLevel"),
                )?,
            },
        })
    }
}

/// A comma-separated list of names, as SecondBot's MastersCSV.
pub fn parse_masters(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .collect()
}

/// "last", "home", or "Region Name/x/y/z" (x, y, z optional, 128/128/0 by default).
pub fn parse_start(s: &str) -> Option<StartLocation> {
    match s.trim().to_ascii_lowercase().as_str() {
        "last" => return Some(StartLocation::Last),
        "home" => return Some(StartLocation::Home),
        "" => return None,
        _ => {}
    }
    let mut parts = s.trim().split('/');
    let name = parts.next()?.trim().to_owned();
    if name.is_empty() {
        return None;
    }
    let mut coord = |default: u32| -> Option<u32> {
        match parts.next() {
            None => Some(default),
            Some(p) => p.trim().parse().ok(),
        }
    };
    let (x, y, z) = (coord(128)?, coord(128)?, coord(0)?);
    Some(StartLocation::Region { name, x, y, z })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(settings: &[(&str, &str)]) -> Result<Vec<CustomCommand>, String> {
        custom_commands_from(|name| {
            settings
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        })
        .map_err(|e| e.to_string())
    }

    #[test]
    fn custom_commands_by_both_names() {
        assert_eq!(custom(&[]), Ok(vec![]), "none by default");
        let all = custom(&[
            ("FERRO_CUSTOM_COMMAND_1_TRIGGER", "greet"),
            ("FERRO_CUSTOM_COMMAND_1_ARGS", "1"),
            ("FERRO_CUSTOM_COMMAND_1_STEP_1", "Say|||0~#~Hi [C_ARG_1]"),
            ("FERRO_CUSTOM_COMMAND_1_STEP_2", "IM|||[C_ARG_1]~#~welcome"),
            // counted until the first one missing
            ("FERRO_CUSTOM_COMMAND_1_STEP_4", "Logoff"),
            ("FERRO_CUSTOM_COMMAND_3_TRIGGER", "skipped"),
            ("customcommands_count", "1"),
            ("customcommands_1_trigger", "where"),
            ("customcommands_1_steps", "1"),
            ("customcommands_1_commands_1", "GetPosition#|#5"),
            ("customcommands_1_commands_2", "Logoff"),
        ])
        .expect("good settings");
        assert_eq!(all.len(), 2);
        assert_eq!(
            (all[0].trigger.as_str(), all[0].args, all[0].steps.len()),
            ("greet", 1, 2)
        );
        assert_eq!(all[0].steps[1].args, ["[C_ARG_1]", "welcome"]);
        assert_eq!((all[1].trigger.as_str(), all[1].args), ("where", 0));
        assert_eq!(
            all[1].steps,
            vec![Step {
                command: "GetPosition".into(),
                args: vec![],
                reply: Some("5".into()),
            }]
        );
    }

    #[test]
    fn custom_commands_that_stop_the_bot() {
        let error = |settings: &[(&str, &str)]| custom(settings).expect_err("refused");
        // SecondBot would run Logoff for the missing step
        assert!(
            error(&[
                ("customcommands_count", "1"),
                ("customcommands_1_trigger", "x"),
                ("customcommands_1_steps", "2"),
                ("customcommands_1_commands_1", "Hello"),
            ])
            .contains("customcommands_1_commands_2 is not set")
        );
        assert!(error(&[("customcommands_count", "1")]).contains("customcommands_1_trigger"));
        assert!(error(&[("FERRO_CUSTOM_COMMAND_1_TRIGGER", "x")]).contains("has no steps"));
        assert!(
            error(&[
                ("FERRO_CUSTOM_COMMAND_1_TRIGGER", "hello"),
                ("FERRO_CUSTOM_COMMAND_1_STEP_1", "Name"),
            ])
            .contains("built-in command Hello")
        );
        assert!(
            error(&[
                ("FERRO_CUSTOM_COMMAND_1_TRIGGER", "x"),
                ("FERRO_CUSTOM_COMMAND_1_STEP_1", "Name"),
                ("customcommands_count", "1"),
                ("customcommands_1_trigger", "x"),
                ("customcommands_1_steps", "1"),
                ("customcommands_1_commands_1", "Name"),
            ])
            .contains("given twice")
        );
        assert!(
            error(&[
                ("FERRO_CUSTOM_COMMAND_1_TRIGGER", "x"),
                ("FERRO_CUSTOM_COMMAND_1_ARGS", "two"),
                ("FERRO_CUSTOM_COMMAND_1_STEP_1", "Name"),
            ])
            .contains("a number of arguments")
        );
        assert!(
            error(&[
                ("FERRO_CUSTOM_COMMAND_1_TRIGGER", "x"),
                ("FERRO_CUSTOM_COMMAND_1_STEP_1", "|||nothing"),
            ])
            .contains("names no command")
        );
        assert!(error(&[("customcommands_count", "many")]).contains("customcommands_count"));
    }

    #[test]
    fn masters_are_a_list_of_names() {
        assert_eq!(
            parse_masters(" Some Resident, other.avatar ,, "),
            vec!["Some Resident".to_owned(), "other.avatar".to_owned()]
        );
        assert!(parse_masters("").is_empty());
    }

    fn smtp_with(
        vars: &[(&str, &str)],
    ) -> Result<Option<crate::services::smtp::Settings>, ConfigError> {
        let vars: std::collections::HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        smtp_from(&move |name| vars.get(name).cloned())
    }

    #[test]
    fn smtp_settings() {
        use crate::services::smtp::Security;
        assert!(smtp_with(&[]).expect("off").is_none());
        assert!(matches!(
            smtp_with(&[("FERRO_SMTP", "true")]),
            Err(ConfigError::Missing("FERRO_SMTP_HOST"))
        ));
        // SecondBot's names, as a SecondBot container has them
        let s = smtp_with(&[
            ("smtp_Enabled", "true"),
            ("smtp_Host", "smtp.example.invalid"),
            ("smtp_Port", "465"),
            ("smtp_useSSL", "true"),
            ("smtp_User", "bot@example.invalid"),
            ("smtp_Token", " pass word "),
            ("smtp_MailReplyAddress", "bot@example.invalid"),
            ("smtp_AllowCommandSendMail", "true"),
            ("smtp_UseAllowedRecivers", "true"),
            (
                "smtp_AllowedReciversCSV",
                "a@example.invalid, b@example.invalid",
            ),
            ("smtp_SendAlertsAndLoginsTo", "me@somewhere.tld"),
        ])
        .expect("valid")
        .expect("on");
        assert_eq!((s.host.as_str(), s.port), ("smtp.example.invalid", 465));
        assert_eq!(s.security, Security::Tls);
        assert_eq!(
            s.login,
            Some(("bot@example.invalid".to_owned(), " pass word ".to_owned()))
        );
        assert!(s.from.is_some() && s.command && !s.reply_target && !s.alerts);
        assert_eq!(
            s.allowed,
            Some(vec![
                "a@example.invalid".to_owned(),
                "b@example.invalid".to_owned()
            ])
        );
        // SecondBot's placeholder: no address
        assert!(s.notify.is_none());
        let shown = format!("{s:?}");
        assert!(
            !shown.contains("pass word") && !shown.contains("bot@"),
            "{shown}"
        );
        // Ferrobot's names; "none" is no login, STARTTLS by default
        let s = smtp_with(&[
            ("FERRO_SMTP", "on"),
            ("FERRO_SMTP_HOST", "smtp.example.invalid"),
            ("FERRO_SMTP_PASSWORD", "none"),
            ("FERRO_SMTP_FROM", "me@myemail.tld"),
        ])
        .expect("valid")
        .expect("on");
        assert_eq!((s.port, s.security), (587, Security::StartTls));
        assert!(s.login.is_none() && s.from.is_none() && s.allowed.is_none());
        assert!(matches!(
            smtp_with(&[
                ("FERRO_SMTP", "on"),
                ("FERRO_SMTP_HOST", "smtp.example.invalid"),
                ("FERRO_SMTP_PASSWORD", "secret"),
            ]),
            Err(ConfigError::Missing("FERRO_SMTP_USER"))
        ));
        assert!(matches!(
            smtp_with(&[
                ("FERRO_SMTP", "on"),
                ("FERRO_SMTP_HOST", "smtp.example.invalid"),
                ("FERRO_SMTP_FROM", "not an address"),
            ]),
            Err(ConfigError::Invalid("FERRO_SMTP_FROM", _))
        ));
        assert!(matches!(
            smtp_with(&[
                ("FERRO_SMTP", "on"),
                ("FERRO_SMTP_HOST", "smtp.example.invalid"),
                ("FERRO_SMTP_TLS", "maybe"),
            ]),
            Err(ConfigError::Invalid("FERRO_SMTP_TLS", _))
        ));
    }

    #[test]
    fn start_locations() {
        assert!(matches!(parse_start("last"), Some(StartLocation::Last)));
        assert!(matches!(parse_start("HOME"), Some(StartLocation::Home)));
        let Some(StartLocation::Region { name, x, y, z }) = parse_start("Some Region/10/20/30")
        else {
            panic!("a region");
        };
        assert_eq!((name.as_str(), x, y, z), ("Some Region", 10, 20, 30));
        let Some(StartLocation::Region { x, y, z, .. }) = parse_start("Some Region") else {
            panic!("a region");
        };
        assert_eq!((x, y, z), (128, 128, 0));
        assert!(parse_start("Some Region/x").is_none());
        assert!(parse_start("").is_none());
    }
}
