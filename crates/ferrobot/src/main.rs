//! Ferrobot: a Second Life bot in a container.

mod config;
mod services;

use config::Config;
use ferro_client::login::{self, LoginError, LoginRequest};
use ferro_client::{Client, End, Network};
use std::time::Duration;
use tokio::sync::watch;

/// Exit codes: 0 logged out on request, 1 bad settings or a lost connection with relog off,
/// 2 a login refusal that retrying can't fix (password, terms of service, 2FA).
#[tokio::main]
async fn main() -> std::process::ExitCode {
    let health_file = std::env::var("FERRO_HEALTH_FILE")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| services::health::DEFAULT_PATH.to_owned());
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return std::process::ExitCode::from(services::health::check(health_file.as_ref()));
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("Ferrobot {}", login::version());
    let cfg = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(e) => {
            log::error!("{e}");
            return std::process::ExitCode::from(1);
        }
    };
    log::debug!("{cfg:?}");

    let (stop_tx, stop_rx) = watch::channel(false);
    tokio::spawn(async move {
        wait_for_stop().await;
        let _ = stop_tx.send(true);
    });

    let http = match reqwest::Client::builder()
        .redirect(login::redirect_policy())
        .user_agent(format!("Ferrobot/{}", login::version()))
        .connect_timeout(Duration::from_secs(15))
        .build()
    {
        Ok(http) => http,
        Err(e) => {
            log::error!("HTTP client: {e}");
            return std::process::ExitCode::from(1);
        }
    };
    let (client, mut network) = Client::new(http.clone());
    tokio::spawn(services::chat_log::run(client.clone()));
    tokio::spawn(services::health::run(client.clone(), health_file.into()));
    tokio::spawn(services::lures::run(client.clone(), cfg.masters.clone()));
    tokio::spawn(services::restart::run(client.clone()));
    tokio::spawn(services::group_invites::run(
        client.clone(),
        cfg.masters.clone(),
    ));
    std::process::ExitCode::from(run(&cfg, &http, &mut network, stop_rx).await)
}

/// Log in, run the session, and log in again when it ends, as the settings say.
async fn run(
    cfg: &Config,
    http: &reqwest::Client,
    network: &mut Network,
    mut stop: watch::Receiver<bool>,
) -> u8 {
    let req = LoginRequest {
        login_uri: cfg.login_uri.clone(),
        username: cfg.username.clone(),
        password: cfg.password.clone(),
        start: cfg.start.clone(),
    };
    let mut failures = 0u32;
    loop {
        if *stop.borrow() {
            return 0;
        }
        log::info!("logging in as {}", cfg.username);
        let wait = match login::login(http, &req).await {
            Ok(resp) => {
                failures = 0;
                log::info!("logged in as {} {}", resp.first_name, resp.last_name);
                if !resp.message.trim().is_empty() {
                    log::info!("message of the day: {}", resp.message.trim());
                }
                match network.session(&resp, stop.clone()).await {
                    End::LoggedOut => {
                        log::info!("logged out");
                        return 0;
                    }
                    End::Kicked(reason) => log::warn!("disconnected by the grid: {reason}"),
                    End::Lost(reason) => log::warn!("connection lost: {reason}"),
                }
                if !cfg.relog {
                    return 1;
                }
                Duration::from_secs(cfg.relog_delay)
            }
            Err(e) if e.permanent() => {
                log::error!("{e}");
                return 2;
            }
            Err(e) => {
                failures += 1;
                // a session still on the grid takes a minute or two to end
                let wait = if e.previous_session() {
                    Duration::from_secs(60)
                } else {
                    Duration::from_secs((30 * u64::from(failures)).min(600))
                };
                log::warn!("login failed: {e}");
                if matches!(e, LoginError::Refused { .. }) && !cfg.relog {
                    return 1;
                }
                wait
            }
        };
        log::info!("logging in again in {} s", wait.as_secs());
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = stop.changed() => return 0,
        }
    }
}

/// Ctrl-C, or SIGTERM from `docker stop`.
async fn wait_for_stop() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
