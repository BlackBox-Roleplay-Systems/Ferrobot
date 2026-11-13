//! Ferrobot: a Second Life bot in a container.

mod api;
mod commands;
mod config;
mod dialogs;
mod discord;
mod download;
mod event_output;
mod grants;
mod history;
mod inventory_updates;
mod masters;
mod on_event;
mod rabbit;
mod services;
mod smart_reply;
mod store;

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
    // shared: a signal stops the bot, and so does the Logoff command
    let stop_tx = std::sync::Arc::new(stop_tx);
    let signal_stop = stop_tx.clone();
    tokio::spawn(async move {
        wait_for_stop().await;
        let _ = signal_stop.send(true);
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
    network.set_timeouts(cfg.timeouts);
    // Each service's subscription is made here, before its task runs and before the login: a
    // task that subscribed when it first ran could miss the first events after the login.
    tokio::spawn(services::chat_log::run(client.clone(), client.subscribe()));
    tokio::spawn(services::health::run(client.clone(), health_file.into()));
    let masters = std::sync::Arc::new(masters::Masters::new(cfg.masters.clone()));
    let homebound = std::sync::Arc::new(services::homebound::Homebound::default());
    let grants = std::sync::Arc::new(grants::Grants::default());
    tokio::spawn(services::script_permissions::run(
        client.clone(),
        client.subscribe(),
        grants.clone(),
    ));
    tokio::spawn(services::interactions::run(
        std::sync::Arc::new(services::interactions::Service {
            client: client.clone(),
            masters: masters.clone(),
            rules: cfg.interactions,
            homebound: homebound.clone(),
            grants: grants.clone(),
            http: http.clone(),
            events: cfg.interaction_events.clone(),
        }),
        client.subscribe(),
    ));
    match cfg.homebound.clone() {
        Some(settings) => {
            log::info!("homebound to {}", settings.home.region);
            tokio::spawn(services::homebound::run(
                client.clone(),
                client.subscribe(),
                settings,
                homebound.clone(),
            ));
        }
        // without homebound, a restart sends the bot to its Second Life home
        None => {
            tokio::spawn(services::restart::run(client.clone(), client.subscribe()));
        }
    }
    let inventory_updates = std::sync::Arc::new(inventory_updates::InventoryUpdates::default());
    tokio::spawn(inventory_updates::run(
        client.clone(),
        client.subscribe(),
        inventory_updates.clone(),
        http.clone(),
        cfg.shared_secret.clone(),
    ));
    if let Some(settings) = cfg.events.clone() {
        log::info!("events service on");
        tokio::spawn(services::events::run(
            client.clone(),
            client.subscribe(),
            settings,
            http.clone(),
            cfg.shared_secret.clone(),
        ));
    }
    let history = std::sync::Arc::new(history::History::default());
    tokio::spawn(history::run(
        client.clone(),
        client.subscribe(),
        history.clone(),
    ));
    // RabbitMQ's handle now, for the relays; its connection once the commands' context is made
    let rabbit = cfg.rabbit.clone().map(|settings| {
        let (handle, outbox) = rabbit::Rabbit::new();
        (handle, outbox, settings)
    });
    tokio::spawn(services::relays::run(
        client.clone(),
        client.subscribe(),
        cfg.relays.clone(),
        cfg.shared_secret.clone(),
        rabbit.as_ref().map(|(handle, ..)| handle.clone()),
    ));
    tokio::spawn(services::chatgpt::run(
        client.clone(),
        client.subscribe(),
        cfg.chatgpt.clone(),
    ));
    let dialogs = std::sync::Arc::new(dialogs::Dialogs::new(cfg.dialog_relay.clone()));
    tokio::spawn(dialogs::run(
        client.clone(),
        client.subscribe(),
        dialogs.clone(),
        cfg.shared_secret.clone(),
    ));
    // subscribed before the login, as every service
    let discord = cfg
        .discord
        .clone()
        .map(|s| discord::Discord::new(s, cfg.shared_secret.clone()));
    if let Some(d) = &discord {
        tokio::spawn(discord::relay::from_world(
            client.clone(),
            d.clone(),
            client.subscribe(),
        ));
    }
    let ctx = commands::Ctx {
        client: client.clone(),
        history,
        dialogs,
        store: std::sync::Arc::new(store::Store::new(cfg.rate_limit)),
        home: cfg.homebound.as_ref().map(|h| h.home.clone()),
        stop: stop_tx.clone(),
        homebound: homebound.clone(),
        allow_funds: cfg.allow_funds,
        http: http.clone(),
        grants: grants.clone(),
        discord: discord.clone(),
        inventory_updates,
        rabbit: rabbit.as_ref().map(|(handle, ..)| handle.clone()),
        custom: std::sync::Arc::new(commands::custom::CustomCommands::new(
            cfg.custom_commands.clone(),
        )),
    };
    if let Some((_, outbox, settings)) = rabbit {
        log::info!(
            "RabbitMQ: {}:{}{}",
            settings.host,
            settings.port,
            if settings.tls { " (TLS)" } else { "" }
        );
        // the queues are read only when their messages can be checked
        let inbound = match &cfg.shared_secret {
            Some(_) => Some(rabbit::inbound::Inbound {
                ctx: ctx.clone(),
                check: std::sync::Arc::new(services::im_commands::Settings {
                    secret: cfg.shared_secret.clone(),
                    enforce_time_window: cfg.enforce_time_window,
                    time_window: cfg.time_window,
                    object_master_optout: false,
                }),
            }),
            None => {
                log::warn!("RabbitMQ: its queues are not read without a shared secret");
                None
            }
        };
        tokio::spawn(rabbit::run(settings, outbox, inbound));
    }
    if !cfg.custom_commands.is_empty() {
        log::info!("{} custom commands", cfg.custom_commands.len());
    }
    if let Some(d) = &discord {
        log::info!("discord: server {}", d.settings.server);
        tokio::spawn(discord::run(ctx.clone(), d.clone()));
    }
    if cfg.im_commands {
        tokio::spawn(services::im_commands::run(
            ctx.clone(),
            client.subscribe(),
            masters.clone(),
            services::im_commands::Settings {
                secret: cfg.shared_secret.clone(),
                enforce_time_window: cfg.enforce_time_window,
                time_window: cfg.time_window,
                object_master_optout: cfg.object_master_optout,
            },
        ));
    }
    if let Some(settings) = cfg.on_event.clone() {
        // subscribed here, before the login, as every service
        tokio::spawn(services::on_event::run(
            ctx.clone(),
            client.subscribe(),
            settings,
        ));
    }
    match (&cfg.shared_secret, cfg.http_enabled) {
        (Some(secret), true) => {
            tokio::spawn(api::serve(
                ctx,
                cfg.http_bind,
                cfg.http_port,
                secret.clone(),
                cfg.time_window,
            ));
        }
        (None, true) => log::warn!("HTTP API off: no shared secret (FERRO_SHARED_SECRET)"),
        (_, false) => log::info!("HTTP API off"),
    }
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
