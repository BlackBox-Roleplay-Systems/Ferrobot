//! The bot's settings, from environment variables (the container's env).

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
            .field("shared_secret_set", &self.shared_secret.is_some())
            .field("time_window", &self.time_window)
            .field("im_commands", &self.im_commands)
            .field("enforce_time_window", &self.enforce_time_window)
            .field("object_master_optout", &self.object_master_optout)
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
