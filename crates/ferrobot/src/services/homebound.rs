//! Homebound, as SecondBot's HomeboundService: the bot goes back to its home sim
//! (`Region/x/y/z`) when it has been away for FERRO_HOME_AFTER minutes, to the backup sim when
//! that fails, and leaves the region for the other one when a restart is announced. At home it
//! goes to its spot when it is more than a few metres away, and sits on its seat
//! (AtHomeAutoSitUuid) when one is given. A master's teleport pauses it until the next login
//! (SecondBot's MarkTeleport); a Stand command stops the sitting until the next login (its
//! MarkStandup, which lasts until SecondBot restarts).

use crate::services::control::Spec;
use ferro_client::{Client, Event, Move};
use glam::Vec3;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

const CHECK_EVERY: Duration = Duration::from_secs(5);
/// Teleports at least this far apart (SecondBot's teleportActionLockout).
const LOCKOUT: Duration = Duration::from_secs(30);
/// At home, nearer than this to the spot is there.
const NEAR: f32 = 5.0;
/// Asks to sit at least this far apart, while the seat does not take the bot.
const SIT_EVERY: Duration = Duration::from_secs(30);

/// A place as SecondBot's SimSlURL reads it: `Region Name/x/y/z` (`%20` for spaces).
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub region: String,
    pub position: Vec3,
}

impl Place {
    pub fn parse(s: &str) -> Option<Place> {
        let s = s.replace("%20", " ");
        let mut parts = s.trim().trim_matches('/').split('/');
        let region = parts.next()?.trim().to_owned();
        if region.is_empty() {
            return None;
        }
        let mut n = |d: f32| -> Option<f32> {
            match parts.next() {
                None => Some(d),
                Some(p) => p.trim().parse().ok(),
            }
        };
        let position = Vec3::new(n(128.0)?, n(128.0)?, n(25.0)?);
        Some(Place { region, position })
    }

    fn is(&self, region: &str) -> bool {
        self.region.eq_ignore_ascii_case(region)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub home: Place,
    pub backup: Option<Place>,
    /// Away this long, the bot goes home (ReturnToHomeSimAfterMins).
    pub after: Duration,
    /// At home, go to the spot (AtHomeSeekLocation).
    pub seek: bool,
    /// At home, sit on this object (AtHomeAutoSitUuid).
    pub sit: Option<uuid::Uuid>,
}

/// What other services tell homebound, and what it tells them.
#[derive(Default)]
pub struct Homebound {
    paused: AtomicBool,
    /// A Stand command: no sitting at home until the next login.
    stood: AtomicBool,
    /// The home sim while homebound runs: GoHome's, and the region restart's watch stands aside.
    home: Mutex<Option<Place>>,
}

impl Homebound {
    /// The home sim, while homebound runs.
    pub fn home(&self) -> Option<Place> {
        self.home_lock().clone()
    }

    fn home_lock(&self) -> std::sync::MutexGuard<'_, Option<Place>> {
        self.home
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A master took the bot elsewhere: stay there until the next login.
    pub fn pause(&self) {
        if !self.paused.swap(true, Ordering::Relaxed) {
            log::info!("homebound paused until the next login: a master's teleport");
        }
    }

    /// The bot was told to stand: it stays standing at home until the next login.
    pub fn stood_up(&self) {
        self.stood.store(true, Ordering::Relaxed);
    }
}

async fn go(client: &Client, place: &Place, why: &str) -> bool {
    log::info!("homebound: {why}, to {}", place.region);
    match client
        .teleport_to_region(&place.region, place.position)
        .await
    {
        Ok(_) => true,
        Err(e) => {
            log::warn!("homebound: could not go to {}: {e}", place.region);
            false
        }
    }
}

/// HomeboundService's settings (SecondBot's HomeboundConfig, `homebound_…`).
const SETTINGS: &[&str] = &[
    "Enabled",
    "HomeSimSlUrl",
    "BackupSimSLUrl",
    "ReturnToHomeSimAfterMins",
    "AtHomeSeekLocation",
    "AtHomeAutoSitUuid",
];

/// HomeboundService, for the service control (services/control.rs). With RLV on it never runs,
/// as SecondBot turns homebound off then: the objects decide where the bot goes.
pub fn spec(client: Client, hb: Arc<Homebound>, rlv: bool) -> Spec {
    Spec::new(
        "HomeboundService",
        "homebound",
        SETTINGS,
        crate::config::homebound,
        move |settings| match settings {
            Some(_) if rlv => {
                log::info!("homebound off: RLV is on");
                Vec::new()
            }
            Some(s) => vec![spawn(client.clone(), s, hb.clone())],
            None => Vec::new(),
        },
    )
}

/// Its home is known while the task runs, and forgotten when it ends or is stopped.
struct Running(Arc<Homebound>);

impl Drop for Running {
    fn drop(&mut self) {
        *self.0.home_lock() = None;
    }
}

/// Homebound's task, subscribed now.
pub fn spawn(client: Client, s: Settings, hb: Arc<Homebound>) -> tokio::task::JoinHandle<()> {
    log::info!("homebound to {}", s.home.region);
    *hb.home_lock() = Some(s.home.clone());
    let running = Running(hb.clone());
    let events = client.subscribe();
    tokio::spawn(async move {
        let _running = running;
        run(client, events, s, hb).await;
    })
}

async fn run(client: Client, mut events: Receiver<Event>, s: Settings, hb: Arc<Homebound>) {
    let mut check = tokio::time::interval(CHECK_EVERY);
    let mut away_since: Option<Instant> = None;
    let mut last_teleport: Option<Instant> = None;
    let mut backup_next = false;
    let mut sought = false;
    let mut last_sit: Option<Instant> = None;
    let mut logins = 0;
    loop {
        tokio::select! {
            ev = events.recv() => match ev {
                Ok(Event::Arrived { .. }) => {
                    let st = client.state();
                    if st.logins != logins {
                        logins = st.logins;
                        hb.paused.store(false, Ordering::Relaxed);
                        hb.stood.store(false, Ordering::Relaxed);
                    }
                    sought = false;
                }
                Ok(Event::RegionRestart { .. }) => {
                    // leave for the other place: the backup from home, home from elsewhere
                    let here = client.state().region.map(|r| r.name).unwrap_or_default();
                    let to = if s.home.is(&here) { s.backup.as_ref() } else { Some(&s.home) };
                    if let Some(to) = to {
                        last_teleport = Some(Instant::now());
                        go(&client, to, "the region restarts").await;
                    }
                }
                Ok(_) => {}
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            },
            _ = check.tick() => {
                let st = client.state();
                let Some(region) = st.region.as_ref().filter(|_| st.online) else {
                    continue;
                };
                if hb.paused.load(Ordering::Relaxed) {
                    continue;
                }
                if s.home.is(&region.name) {
                    away_since = None;
                    backup_next = false;
                    // seated, the bot is where its seat is: no seeking the spot
                    let seated = st.sitting_on.is_some();
                    if s.seek && !sought && !seated && st.position.distance(s.home.position) > NEAR {
                        sought = true;
                        go(&client, &s.home, "to the spot at home").await;
                        continue;
                    }
                    let sit_due = last_sit.is_none_or(|t| t.elapsed() >= SIT_EVERY);
                    if let Some(seat) = s.sit
                        && !seated
                        && sit_due
                        && !hb.stood.load(Ordering::Relaxed)
                    {
                        last_sit = Some(Instant::now());
                        log::info!("homebound: sitting on {seat} at home");
                        if let Err(e) = client.move_bot(Move::SitOn(seat)).await {
                            log::warn!("homebound: could not sit: {e}");
                        }
                    }
                    continue;
                }
                let away = *away_since.get_or_insert_with(Instant::now);
                let locked = last_teleport.is_some_and(|t| t.elapsed() < LOCKOUT);
                if away.elapsed() < s.after || locked {
                    continue;
                }
                last_teleport = Some(Instant::now());
                let target = match (&s.backup, backup_next) {
                    (Some(b), true) if !b.is(&region.name) => b,
                    _ => &s.home,
                };
                let ok = go(&client, target, "away from home").await;
                // home failed: the backup next time; the backup failed: home again
                backup_next = !ok && std::ptr::eq(target, &s.home);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_as_secondbot_reads_them() {
        assert_eq!(
            Place::parse("Some%20Sim/128/120/27"),
            Some(Place {
                region: "Some Sim".into(),
                position: Vec3::new(128.0, 120.0, 27.0),
            })
        );
        assert_eq!(
            Place::parse("Viserion").map(|p| p.position),
            Some(Vec3::new(128.0, 128.0, 25.0))
        );
        assert!(Place::parse("Region/x/1/2").is_none());
        assert!(Place::parse("").is_none());
    }
}
