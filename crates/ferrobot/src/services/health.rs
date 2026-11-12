//! The health file: the bot writes its health every few seconds, and `ferrobot healthcheck`
//! (Docker's HEALTHCHECK) reads it. No HTTP server needed (the API comes in ROADMAP phase 4).
//!
//! Healthy: the file is fresh (the bot is not stuck) and the bot is online, or has been offline
//! for less than [`OFFLINE_GRACE`] (a relog in progress).

use ferro_client::Client;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const WRITE_EVERY: Duration = Duration::from_secs(5);
/// Older than this, the file means the bot stopped writing it.
const STALE: Duration = Duration::from_secs(30);
/// Offline this long: unhealthy (a relog takes a minute or two, a grid problem may take more).
pub const OFFLINE_GRACE: Duration = Duration::from_secs(600);

pub const DEFAULT_PATH: &str = "/tmp/ferrobot-health";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Record {
    updated: u64,
    online: bool,
    /// The last time the bot was seen online, or its start.
    last_online: u64,
}

impl Record {
    fn to_text(self) -> String {
        format!(
            "updated={}\nonline={}\nlast_online={}\n",
            self.updated, self.online, self.last_online
        )
    }

    fn parse(text: &str) -> Option<Record> {
        let mut updated = None;
        let mut online = None;
        let mut last_online = None;
        for line in text.lines() {
            match line.split_once('=') {
                Some(("updated", v)) => updated = v.trim().parse().ok(),
                Some(("online", v)) => online = v.trim().parse().ok(),
                Some(("last_online", v)) => last_online = v.trim().parse().ok(),
                _ => {}
            }
        }
        Some(Record {
            updated: updated?,
            online: online?,
            last_online: last_online?,
        })
    }

    /// Why the bot is unhealthy, or `None` when it is healthy.
    fn problem(&self, now: u64) -> Option<String> {
        let age = now.saturating_sub(self.updated);
        if age > STALE.as_secs() {
            return Some(format!("the health file is {age} s old: the bot is stuck"));
        }
        let offline = now.saturating_sub(self.last_online);
        if !self.online && offline > OFFLINE_GRACE.as_secs() {
            return Some(format!("offline for {offline} s"));
        }
        None
    }
}

/// Write the health file until the bot stops.
pub async fn run(client: Client, path: PathBuf) {
    let mut last_online = now();
    let mut warned = false;
    loop {
        let online = client.health().online;
        if online {
            last_online = now();
        }
        let record = Record {
            updated: now(),
            online,
            last_online,
        };
        if let Err(e) = write(&path, &record.to_text()) {
            if !warned {
                log::warn!("health file {}: {e}", path.display());
                warned = true;
            }
        } else {
            warned = false;
        }
        tokio::time::sleep(WRITE_EVERY).await;
    }
}

/// Written beside, then renamed: a reader never sees half a file.
fn write(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// `ferrobot healthcheck`: exit code 0 when healthy, 1 when not, with the reason on stderr.
pub fn check(path: &Path) -> u8 {
    let problem = match std::fs::read_to_string(path) {
        Ok(text) => match Record::parse(&text) {
            Some(r) => r.problem(now()),
            None => Some("the health file is malformed".into()),
        },
        Err(e) => Some(format!("no health file at {}: {e}", path.display())),
    };
    match problem {
        None => 0,
        Some(why) => {
            eprintln!("unhealthy: {why}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 1_800_000_000;

    #[test]
    fn the_record_reads_back() {
        let r = Record {
            updated: T,
            online: true,
            last_online: T - 3,
        };
        assert_eq!(Record::parse(&r.to_text()), Some(r));
        assert_eq!(Record::parse("updated=1\n"), None);
    }

    #[test]
    fn healthy_online_or_briefly_offline() {
        let online = Record {
            updated: T,
            online: true,
            last_online: T,
        };
        assert_eq!(online.problem(T + 5), None);
        let relogging = Record {
            online: false,
            last_online: T - 120,
            ..online
        };
        assert_eq!(relogging.problem(T + 5), None);
    }

    #[test]
    fn unhealthy_when_stuck_or_offline_too_long() {
        let r = Record {
            updated: T,
            online: true,
            last_online: T,
        };
        assert!(r.problem(T + 31).expect("stuck").contains("stuck"));
        let offline = Record {
            online: false,
            last_online: T - 601,
            ..r
        };
        assert!(offline.problem(T).expect("offline").contains("offline"));
    }
}
