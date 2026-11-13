//! The bot's settings, from environment variables (the container's env).

use crate::services::interactions::{Level, Rule, Rules};
use ferro_client::login::{SL_MAIN_GRID, StartLocation};

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
            .field("dialog_relay", &self.dialog_relay)
            .field("homebound", &self.homebound)
            .field("rate_limit", &self.rate_limit)
            .field("allow_funds", &self.allow_funds)
            .field("relays", &self.relays.len())
            .field("discord", &self.discord)
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
            relays: relays()?,
            allow_funds: setting_flag("FERRO_ALLOW_FUNDS", "commands_AllowFundsCommands", false)?,
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

    #[test]
    fn masters_are_a_list_of_names() {
        assert_eq!(
            parse_masters(" Some Resident, other.avatar ,, "),
            vec!["Some Resident".to_owned(), "other.avatar".to_owned()]
        );
        assert!(parse_masters("").is_empty());
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
