//! Homebound, as SecondBot's HomeboundService: the bot goes back to its home sim
//! (`Region/x/y/z`) when it has been away for FERRO_HOME_AFTER minutes, to the backup sim when
//! that fails, and leaves the region for the other one when a restart is announced. At home it
//! goes to its spot when it is more than a few metres away. A master's teleport pauses it until
//! the next login (SecondBot's MarkTeleport).
//!
//! Not here yet: sitting on an object at home (AtHomeAutoSitUuid), with movement (phase 7).

use ferro_client::{Client, Event};
use glam::Vec3;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

const CHECK_EVERY: Duration = Duration::from_secs(5);
/// Teleports at least this far apart (SecondBot's teleportActionLockout).
const LOCKOUT: Duration = Duration::from_secs(30);
/// At home, nearer than this to the spot is there.
const NEAR: f32 = 5.0;

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
}

/// What other services tell homebound.
#[derive(Default)]
pub struct Homebound {
    paused: AtomicBool,
}

impl Homebound {
    /// A master took the bot elsewhere: stay there until the next login.
    pub fn pause(&self) {
        if !self.paused.swap(true, Ordering::Relaxed) {
            log::info!("homebound paused until the next login: a master's teleport");
        }
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

pub async fn run(client: Client, mut events: Receiver<Event>, s: Settings, hb: Arc<Homebound>) {
    let mut check = tokio::time::interval(CHECK_EVERY);
    let mut away_since: Option<Instant> = None;
    let mut last_teleport: Option<Instant> = None;
    let mut backup_next = false;
    let mut sought = false;
    let mut logins = 0;
    loop {
        tokio::select! {
            ev = events.recv() => match ev {
                Ok(Event::Arrived { .. }) => {
                    let st = client.state();
                    if st.logins != logins {
                        logins = st.logins;
                        hb.paused.store(false, Ordering::Relaxed);
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
                    if s.seek && !sought && st.position.distance(s.home.position) > NEAR {
                        sought = true;
                        go(&client, &s.home, "to the spot at home").await;
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
            Place::parse("Sandbox%20Decorus/128/120/27"),
            Some(Place {
                region: "Sandbox Decorus".into(),
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
