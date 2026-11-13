//! What the bot keeps for its commands besides the chat history: SecondBot's key-value store
//! (ReadKeyValue, SetKeyValue, ClearKeyValue), its command history (GetLastCommands), the
//! counts behind BotStatus, and the rate limit of the commands that change things.

use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// CommandHistoryLimit (DataStoreConfig).
const HISTORY_LIMIT: usize = 30;

/// One command run (SecondBot's CommandHistory, its fields in their order).
#[derive(Debug, Clone, Serialize)]
#[allow(non_snake_case)]
pub struct CommandRecord {
    pub Command: String,
    pub Args: Vec<String>,
    pub Unixtime: i64,
    pub status: bool,
    pub results: Option<String>,
}

#[derive(Default)]
struct Inner {
    kv: HashMap<String, String>,
    history: VecDeque<CommandRecord>,
    commands: u64,
    failures: u64,
    /// The rate limit's tokens and when they were counted.
    tokens: f64,
    counted: Option<Instant>,
}

pub struct Store {
    inner: Mutex<Inner>,
    pub started: Instant,
    /// Commands that change things, per minute (0: no limit), and the burst allowed.
    per_minute: u32,
    burst: u32,
}

impl Store {
    pub fn new(per_minute: u32) -> Self {
        Store {
            inner: Mutex::new(Inner {
                tokens: f64::from(per_minute.min(20)),
                ..Inner::default()
            }),
            started: Instant::now(),
            per_minute,
            burst: per_minute.min(20),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn get(&self, key: &str) -> String {
        self.lock().kv.get(key).cloned().unwrap_or_default()
    }

    pub fn set(&self, key: &str, value: &str) {
        self.lock().kv.insert(key.to_owned(), value.to_owned());
    }

    /// AppendKeyValue: the value added at the end; answers the new length.
    pub fn append(&self, key: &str, value: &str) -> usize {
        let mut i = self.lock();
        let v = i.kv.entry(key.to_owned()).or_default();
        v.push_str(value);
        v.len()
    }

    pub fn clear(&self, key: &str) {
        self.lock().kv.remove(key);
    }

    pub fn record(&self, r: CommandRecord) {
        let mut i = self.lock();
        i.commands += 1;
        if !r.status {
            i.failures += 1;
        }
        i.history.push_back(r);
        while i.history.len() > HISTORY_LIMIT {
            i.history.pop_front();
        }
    }

    /// The most recent commands, newest first.
    pub fn last(&self, n: usize) -> Vec<CommandRecord> {
        self.lock().history.iter().rev().take(n).cloned().collect()
    }

    pub fn counts(&self) -> (u64, u64) {
        let i = self.lock();
        (i.commands, i.failures)
    }

    /// Wait for the rate limit to let a command that changes things through; false when that
    /// would take longer than `patience`.
    pub async fn admit(&self, patience: Duration) -> bool {
        if self.per_minute == 0 {
            return true;
        }
        let rate = f64::from(self.per_minute) / 60.0;
        let wait = {
            let mut i = self.lock();
            let now = Instant::now();
            let elapsed = i
                .counted
                .map_or(0.0, |t| now.duration_since(t).as_secs_f64());
            i.tokens = (i.tokens + elapsed * rate).min(f64::from(self.burst.max(1)));
            i.counted = Some(now);
            // take the token now, even if it is not there yet: the next one waits longer
            i.tokens -= 1.0;
            if i.tokens >= 0.0 {
                Duration::ZERO
            } else {
                Duration::from_secs_f64(-i.tokens / rate)
            }
        };
        if wait > patience {
            self.lock().tokens += 1.0;
            return false;
        }
        tokio::time::sleep(wait).await;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_history_keeps_the_last_commands() {
        let s = Store::new(0);
        for i in 0..40 {
            s.record(CommandRecord {
                Command: format!("C{i}"),
                Args: vec![],
                Unixtime: i,
                status: i % 2 == 0,
                results: None,
            });
        }
        let last = s.last(5);
        assert_eq!(last[0].Command, "C39");
        assert_eq!(last.len(), 5);
        assert_eq!(s.counts(), (40, 20));
        assert_eq!(
            serde_json::to_string(&last[0]).expect("json"),
            r#"{"Command":"C39","Args":[],"Unixtime":39,"status":false,"results":null}"#
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_rate_limit_lets_a_burst_through_then_waits() {
        let s = Store::new(60);
        for _ in 0..20 {
            assert!(s.admit(Duration::ZERO).await, "the burst");
        }
        assert!(!s.admit(Duration::ZERO).await, "no patience, no token");
        let t = tokio::time::Instant::now();
        assert!(s.admit(Duration::from_secs(5)).await);
        assert!(
            t.elapsed() >= Duration::from_millis(900),
            "a token a second"
        );
        assert!(Store::new(0).admit(Duration::ZERO).await, "no limit");
    }
}
