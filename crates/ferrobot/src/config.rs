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
}

impl std::fmt::Debug for Config {
    // never prints the password
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("login_uri", &self.login_uri)
            .field("username", &self.username)
            .field("start", &self.start)
            .field("relog", &self.relog)
            .field("relog_delay", &self.relog_delay)
            .field("masters", &self.masters)
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

fn var(name: &'static str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn flag(name: &'static str, default: bool) -> Result<bool, ConfigError> {
    match var(name).map(|v| v.to_ascii_lowercase()) {
        None => Ok(default),
        Some(v) if matches!(v.as_str(), "1" | "true" | "yes" | "on") => Ok(true),
        Some(v) if matches!(v.as_str(), "0" | "false" | "no" | "off") => Ok(false),
        Some(v) => Err(ConfigError::Invalid(
            name,
            format!("\"{v}\" is not true or false"),
        )),
    }
}

impl Config {
    /// FERRO_USERNAME, FERRO_PASSWORD (required); FERRO_LOGIN_URI (Second Life by default), FERRO_START
    /// ("last", "home" or "Region Name/x/y/z"), FERRO_RELOG (true), FERRO_RELOG_DELAY (60 s),
    /// FERRO_MASTERS (none).
    pub fn from_env() -> Result<Config, ConfigError> {
        let start = match var("FERRO_START") {
            None => StartLocation::Last,
            Some(s) => parse_start(&s).ok_or(ConfigError::Invalid(
                "FERRO_START",
                "use last, home or Region Name/x/y/z".into(),
            ))?,
        };
        let relog_delay = match var("FERRO_RELOG_DELAY") {
            None => 60,
            Some(s) => s.parse().map_err(|_| {
                ConfigError::Invalid("FERRO_RELOG_DELAY", "a number of seconds".into())
            })?,
        };
        Ok(Config {
            login_uri: var("FERRO_LOGIN_URI").unwrap_or_else(|| SL_MAIN_GRID.to_owned()),
            username: var("FERRO_USERNAME").ok_or(ConfigError::Missing("FERRO_USERNAME"))?,
            password: std::env::var("FERRO_PASSWORD")
                .ok()
                .filter(|p| !p.is_empty())
                .ok_or(ConfigError::Missing("FERRO_PASSWORD"))?,
            start,
            relog: flag("FERRO_RELOG", true)?,
            relog_delay,
            masters: var("FERRO_MASTERS")
                .map(|v| parse_masters(&v))
                .unwrap_or_default(),
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
