//! XML-RPC login (`login_to_simulator`).
//!
//! Port of aurora-net/src/login.rs (Aurora Viewer, see NOTICE.md), itself modelled on
//! `newview/lllogininstance.cpp`. Trimmed for a bot: no viewer device probe, and a login is only
//! sent on to the same server (or from one Linden Lab login server to another), never elsewhere.

use crate::xmlrpc;
use ferro_llsd::{Llsd, Map};
use std::net::Ipv4Addr;
use uuid::Uuid;

pub const SL_MAIN_GRID: &str = "https://login.agni.lindenlab.com/cgi-bin/login.cgi";

pub const CHANNEL: &str = "Ferrobot";

#[derive(Debug, Clone)]
pub enum StartLocation {
    Last,
    Home,
    Region {
        name: String,
        x: u32,
        y: u32,
        z: u32,
    },
}

impl StartLocation {
    pub fn to_login_string(&self) -> String {
        match self {
            StartLocation::Last => "last".into(),
            StartLocation::Home => "home".into(),
            StartLocation::Region { name, x, y, z } => format!("uri:{name}&{x}&{y}&{z}"),
        }
    }
}

#[derive(Clone)]
pub struct LoginRequest {
    pub login_uri: String,
    /// "First Last" or a single username (last name "Resident").
    pub username: String,
    pub password: String,
    pub start: StartLocation,
}

#[derive(Debug, Clone)]
pub struct LoginResponse {
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub first_name: String,
    pub last_name: String,
    pub sim_ip: Ipv4Addr,
    pub sim_port: u16,
    pub circuit_code: u32,
    pub seed_capability: String,
    pub message: String,
    /// Where home is, when the grid says.
    pub home: Option<crate::state::Home>,
    /// For the inventory and group commands to come.
    #[allow(dead_code)]
    pub secure_session_id: Uuid,
    #[allow(dead_code)]
    pub inventory_root: Uuid,
}

#[derive(Debug, thiserror::Error)]
pub enum LoginError {
    #[error("network error: {0}")]
    Http(String),
    #[error("{0}")]
    XmlRpc(#[from] xmlrpc::XmlRpcError),
    #[error("login refused ({reason}): {message}")]
    Refused {
        reason: String,
        message: String,
        /// `message_id` of the extended errors (`LoginFailedPendingLogout`…).
        message_id: String,
    },
    #[error("two-factor authentication is on for this account; a bot can't answer it: {0}")]
    MfaRequired(String),
    #[error("the terms of service must be accepted first - log in once with a viewer: {0}")]
    TosRequired(String),
    #[error("a critical message must be read first - log in once with a viewer: {0}")]
    CriticalMessage(String),
    #[error("malformed login response: {0}")]
    Malformed(&'static str),
    #[error("the login server sent the login on to another server ({0}); not followed")]
    OtherServer(String),
}

impl LoginError {
    /// The grid still holds a session of this account (reason "presence" and the
    /// LoginFailedPendingLogout family, LLLoginInstance::handleLoginFailure): wait and try again.
    pub fn previous_session(&self) -> bool {
        match self {
            LoginError::Refused {
                reason, message_id, ..
            } => {
                reason == "presence"
                    || matches!(
                        message_id.as_str(),
                        "LoginFailedPendingLogout"
                            | "LoginFailedLoggingOutSession"
                            | "LoginFailedAlreadyLoggedIn"
                            | "LogoutFailedLoggingOut"
                            | "LogoutFailedStillLoggingOut"
                            | "LogoutFailedLogoutBegun"
                            | "LogoutFailedPendingLogin"
                            | "LogoutSucceeded"
                    )
            }
            _ => false,
        }
    }

    /// Trying again can't help until someone changes something (password, terms, 2FA).
    pub fn permanent(&self) -> bool {
        match self {
            LoginError::Refused { reason, .. } => reason == "key" || reason == "god",
            LoginError::MfaRequired(_)
            | LoginError::TosRequired(_)
            | LoginError::CriticalMessage(_) => true,
            LoginError::OtherServer(_) => true,
            _ => false,
        }
    }
}

fn refusal(v: &Llsd) -> LoginError {
    let reason = v["reason"].as_str().to_owned();
    let message = v["message"].as_str().to_owned();
    let message_id = v["message_id"].as_str().to_owned();
    match reason.as_str() {
        "mfa_challenge" => LoginError::MfaRequired(message),
        "tos" => LoginError::TosRequired(message),
        "critical" => LoginError::CriticalMessage(message),
        _ => LoginError::Refused {
            reason,
            message,
            message_id,
        },
    }
}

fn split_username(username: &str) -> (String, String) {
    let t = username.trim();
    let mut parts = t.split([' ', '.']).filter(|s| !s.is_empty());
    let first = parts.next().unwrap_or("").to_owned();
    let last = parts.next().unwrap_or("Resident").to_owned();
    (first, last)
}

/// `$1$` + md5 hex of the password (Second Life truncates passwords to 16 chars).
pub fn hash_password(password: &str, truncate16: bool) -> String {
    if password.len() == 35 && password.starts_with("$1$") {
        return password.to_owned();
    }
    let p: String = if truncate16 {
        password.chars().take(16).collect()
    } else {
        password.to_owned()
    };
    format!("$1${:x}", md5::compute(p.as_bytes()))
}

/// The login server's host, lower case, with its port when one is given.
fn login_authority(uri: &str) -> Option<String> {
    let url = reqwest::Url::parse(uri.trim()).ok()?;
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

/// A Linden Lab login server: its host is lindenlab.com or one of its subdomains.
pub fn is_linden_login(uri: &str) -> bool {
    login_authority(uri).is_some_and(|authority| {
        let host = authority.split(':').next().unwrap_or("");
        host == "lindenlab.com" || host.ends_with(".lindenlab.com")
    })
}

/// Whether the login may go on from `from` to `to`: the same server (http to https included) or
/// one Linden Lab login server to another, and never from encrypted to unencrypted.
fn may_follow(from: &str, to: &str) -> bool {
    let (Ok(from_url), Ok(to_url)) = (
        reqwest::Url::parse(from.trim()),
        reqwest::Url::parse(to.trim()),
    ) else {
        return false;
    };
    if !matches!(to_url.scheme(), "https" | "http")
        || (from_url.scheme() == "https" && to_url.scheme() != "https")
    {
        return false;
    }
    match (login_authority(from), login_authority(to)) {
        (Some(here), Some(there)) => {
            here == there || (is_linden_login(from) && is_linden_login(to))
        }
        _ => false,
    }
}

/// HTTP redirects are followed only where `may_follow` allows.
pub fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        let follow = attempt
            .previous()
            .last()
            .is_some_and(|from| may_follow(from.as_str(), attempt.url().as_str()));
        if follow {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

/// A device id for this account: a bot in a container has no machine of its own to name, so it
/// is derived from the account, steady across restarts. It is sent as the hash viewers send.
fn device_id(kind: &str, username: &str) -> String {
    format!(
        "{:x}",
        md5::compute(format!(
            "ferrobot\n{kind}\n{}",
            username.trim().to_ascii_lowercase()
        ))
    )
}

/// The kernel release ("6.8.0-45-generic") as "6.8.0", or "" when unknown.
fn linux_version() -> String {
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    let numbers: Vec<&str> = release
        .trim()
        .split(|c: char| !c.is_ascii_digit())
        .take_while(|part| !part.is_empty())
        .take(3)
        .collect();
    match numbers.as_slice() {
        [] => String::new(),
        [a] => format!("{a}.0.0"),
        [a, b] => format!("{a}.{b}.0"),
        [a, b, c, ..] => format!("{a}.{b}.{c}"),
    }
}

/// "major.minor.patch.build", the form login servers expect.
pub fn version() -> String {
    format!("{}.0", env!("CARGO_PKG_VERSION"))
}

fn build_params(req: &LoginRequest) -> Llsd {
    let (first, last) = split_username(&req.username);
    let mut p = Map::new();
    p.insert("first".into(), first.into());
    p.insert("last".into(), last.into());
    p.insert(
        "passwd".into(),
        hash_password(&req.password, is_linden_login(&req.login_uri)).into(),
    );
    p.insert("start".into(), req.start.to_login_string().into());
    p.insert("channel".into(), CHANNEL.into());
    p.insert("version".into(), version().into());
    p.insert("platform".into(), "lnx".into());
    let platform_version = linux_version();
    if !platform_version.is_empty() {
        p.insert("platform_version".into(), platform_version.into());
    }
    p.insert("platform_string".into(), "Linux".into());
    p.insert("address_size".into(), 64.into());
    p.insert("mac".into(), device_id("mac", &req.username).into());
    p.insert("id0".into(), device_id("id0", &req.username).into());
    p.insert("host_id".into(), "".into());
    p.insert("agree_to_tos".into(), false.into());
    p.insert("read_critical".into(), false.into());
    p.insert("last_exec_event".into(), 0.into());
    p.insert("last_exec_duration".into(), (-1).into());
    p.insert(
        "last_exec_session_id".into(),
        Uuid::nil().to_string().into(),
    );
    p.insert("extended_errors".into(), true.into());
    p.insert("token".into(), "".into());
    p.insert("mfa_hash".into(), "".into());
    // What a bot needs: its inventory root, its buddies and display names.
    let options: Vec<Llsd> = [
        "inventory-root",
        "buddy-list",
        "display_names",
        "adult_compliant",
        "max-agent-groups",
        "login-flags",
    ]
    .iter()
    .map(|s| Llsd::from(*s))
    .collect();
    p.insert("options".into(), Llsd::Array(options));
    Llsd::Map(p)
}

fn parse_success(v: &Llsd) -> Result<LoginResponse, LoginError> {
    let uuid = |k: &str| v[k].as_uuid();
    let agent_id = uuid("agent_id");
    if agent_id.is_nil() {
        return Err(LoginError::Malformed("agent_id"));
    }
    let sim_ip: Ipv4Addr = v["sim_ip"]
        .as_str()
        .parse()
        .map_err(|_| LoginError::Malformed("sim_ip"))?;
    let sim_port =
        u16::try_from(v["sim_port"].as_i32()).map_err(|_| LoginError::Malformed("sim_port"))?;
    Ok(LoginResponse {
        agent_id,
        session_id: uuid("session_id"),
        secure_session_id: uuid("secure_session_id"),
        first_name: v["first_name"].as_str().trim_matches('"').to_owned(),
        last_name: v["last_name"].as_str().trim_matches('"').to_owned(),
        sim_ip,
        sim_port,
        circuit_code: v["circuit_code"].as_u32(),
        seed_capability: v["seed_capability"].as_str().to_owned(),
        message: v["message"].as_str().to_owned(),
        inventory_root: v["inventory-root"][0]["folder_id"].as_uuid(),
        home: parse_home(v["home"].as_str()),
    })
}

/// The login reply's `home`, LLSD notation in a string:
/// `{'region_handle':[r256000, r256000], 'position':[r128, r128, r25], 'look_at':[r1, r0, r0]}`
/// (LLStartUp's "home" handling, LibreMetaverse's LoginResponseData).
fn parse_home(s: &str) -> Option<crate::state::Home> {
    if s.trim().is_empty() {
        return None;
    }
    let v = ferro_llsd::from_notation(s.as_bytes()).ok()?;
    let vec3 = |l: &Llsd| glam::Vec3::new(l[0].as_f32(), l[1].as_f32(), l[2].as_f32());
    let handle = &v["region_handle"];
    let (x, y) = (handle[0].as_f64(), handle[1].as_f64());
    if x <= 0.0 || y <= 0.0 {
        return None;
    }
    Some(crate::state::Home {
        region_handle: crate::state::region_handle(x as u32, y as u32),
        position: vec3(&v["position"]),
        look_at: vec3(&v["look_at"]),
    })
}

/// The login exchange, following "indeterminate" answers and redirects where `may_follow` allows.
/// `http` must be built with `redirect_policy`.
pub async fn login(
    http: &reqwest::Client,
    req: &LoginRequest,
) -> Result<LoginResponse, LoginError> {
    let mut uri = req.login_uri.clone();
    let mut method = "login_to_simulator".to_owned();
    let params = build_params(req);
    for _attempt in 0..5 {
        let body = xmlrpc::build_call(&method, &params);
        let resp = http
            .post(&uri)
            .header("Content-Type", "text/xml")
            .body(body)
            .send()
            .await
            .map_err(|e| LoginError::Http(e.without_url().to_string()))?;
        let status = resp.status();
        let at = resp.url().clone();
        if matches!(status.as_u16(), 307 | 308)
            && let Some(next) = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|location| location.to_str().ok())
                .and_then(|location| at.join(location).ok())
        {
            if !may_follow(at.as_str(), next.as_str()) {
                return Err(LoginError::OtherServer(
                    login_authority(next.as_str()).unwrap_or_default(),
                ));
            }
            uri = next.into();
            continue;
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| LoginError::Http(e.without_url().to_string()))?;
        if !status.is_success() {
            return Err(LoginError::Http(format!("HTTP {status}")));
        }
        let v = xmlrpc::parse_response(&bytes)?;
        match v["login"].to_string_value().as_str() {
            "true" => return parse_success(&v),
            "indeterminate" => {
                let next = v["next_url"].as_str();
                if next.is_empty() {
                    return Err(LoginError::Malformed("indeterminate without next_url"));
                }
                let next = at
                    .join(next)
                    .map_err(|_| LoginError::Malformed("next_url"))?;
                if !may_follow(at.as_str(), next.as_str()) {
                    return Err(LoginError::OtherServer(
                        login_authority(next.as_str()).unwrap_or_default(),
                    ));
                }
                uri = next.into();
                let m = v["next_method"].as_str();
                if !m.is_empty() {
                    method = m.to_owned();
                }
            }
            _ => return Err(refusal(&v)),
        }
    }
    Err(LoginError::Malformed("too many redirects"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_from_the_login_reply() {
        let home = parse_home(
            "{'region_handle':[r256000, r255744], 'position':[r33.5, r40, r22.25], 'look_at':[r0, r1, r0]}",
        )
        .expect("a home");
        assert_eq!(home.region_handle, (256000u64 << 32) | 255744);
        assert_eq!(home.position, glam::Vec3::new(33.5, 40.0, 22.25));
        assert_eq!(home.look_at, glam::Vec3::Y);
        assert!(parse_home("").is_none());
        assert!(parse_home("not notation").is_none());
    }

    #[test]
    fn username_split() {
        assert_eq!(split_username("jane"), ("jane".into(), "Resident".into()));
        assert_eq!(split_username("Jane Doe"), ("Jane".into(), "Doe".into()));
        assert_eq!(split_username("jane.doe"), ("jane".into(), "doe".into()));
    }

    #[test]
    fn password_hash() {
        assert_eq!(
            hash_password("test", true),
            "$1$098f6bcd4621d373cade4e832627b4f6"
        );
        assert_eq!(
            hash_password("0123456789abcdefXYZ", true),
            hash_password("0123456789abcdef", true)
        );
    }

    #[test]
    fn refusals() {
        let answer = |reason: &str, id: &str| {
            let mut m = Map::new();
            m.insert("login".into(), "false".into());
            m.insert("reason".into(), reason.into());
            m.insert("message".into(), "synthetic".into());
            m.insert("message_id".into(), id.into());
            refusal(&Llsd::Map(m))
        };
        assert!(answer("presence", "").previous_session());
        assert!(answer("other", "LoginFailedPendingLogout").previous_session());
        assert!(!answer("key", "LoginFailedAuthenticationFailed").previous_session());
        assert!(answer("key", "").permanent());
        assert!(answer("tos", "").permanent());
        assert!(!answer("presence", "").permanent());
    }

    #[test]
    fn following_stays_on_the_same_server() {
        assert!(may_follow(
            "https://grid.example:8002/a",
            "https://GRID.example:8002/b"
        ));
        assert!(may_follow("http://grid.example/", "https://grid.example/"));
        assert!(may_follow(
            SL_MAIN_GRID,
            "https://login2.lindenlab.com/cgi-bin/login.cgi"
        ));
        assert!(!may_follow(
            "https://grid.example/",
            "https://other.example/"
        ));
        assert!(!may_follow("https://grid.example/", "http://grid.example/"));
        assert!(!may_follow(
            SL_MAIN_GRID,
            "https://lindenlab.com.other.example/"
        ));
        assert!(!may_follow("https://grid.example/", "ftp://grid.example/"));
    }

    #[test]
    fn device_ids_are_steady_per_account() {
        assert_eq!(device_id("mac", "Some Bot"), device_id("mac", " some bot "));
        assert_ne!(device_id("mac", "Some Bot"), device_id("id0", "Some Bot"));
        assert_ne!(device_id("mac", "Some Bot"), device_id("mac", "Other Bot"));
        assert_eq!(device_id("mac", "x").len(), 32);
    }

    #[test]
    fn login_params() {
        let req = LoginRequest {
            login_uri: SL_MAIN_GRID.into(),
            username: "synthetic".into(),
            password: "x".into(),
            start: StartLocation::Home,
        };
        let p = build_params(&req);
        assert_eq!(p["first"].as_str(), "synthetic");
        assert_eq!(p["last"].as_str(), "Resident");
        assert_eq!(p["channel"].as_str(), CHANNEL);
        assert_eq!(p["start"].as_str(), "home");
        assert_eq!(p["passwd"].as_str(), hash_password("x", true));
        assert_eq!(version().split('.').count(), 4);
    }
}
