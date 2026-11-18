//! Faults a test platform puts into the client: messages the network loses, without a network
//! that loses them. Only with the `faults` feature (ferro-probe turns it on; the bot never does).
//!
//! A dropped event queue event is gone for good: its poll was answered, as when the viewer misses
//! one. A dropped UDP message is not acknowledged: the region resends a reliable one, as after a
//! real loss, and an unreliable one is gone.

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Default)]
pub struct Faults {
    /// How many more of each event to drop, by message name.
    events: Mutex<HashMap<String, u32>>,
    /// How many more of each UDP message to drop, by message name.
    messages: Mutex<HashMap<String, u32>>,
}

impl Faults {
    /// The next `count` event queue events named `message` (CrossedRegion, TeleportFinish…) are
    /// dropped. 0 drops none.
    pub fn drop_events(&self, message: &str, count: u32) {
        set(&self.events, message, count);
    }

    /// The next `count` UDP messages named `message` (AgentMovementComplete, TeleportLocal…) are
    /// dropped, unacknowledged. 0 drops none.
    pub fn drop_messages(&self, message: &str, count: u32) {
        set(&self.messages, message, count);
    }

    /// Drops still to come, events and messages, by name.
    pub fn pending(&self) -> Vec<(String, u32)> {
        let mut v: Vec<(String, u32)> = lock(&self.events)
            .iter()
            .chain(lock(&self.messages).iter())
            .filter(|(_, n)| **n > 0)
            .map(|(k, n)| (k.clone(), *n))
            .collect();
        v.sort();
        v
    }

    pub(crate) fn take_event(&self, message: &str) -> bool {
        take(&self.events, message)
    }

    pub(crate) fn take_message(&self, message: &str) -> bool {
        take(&self.messages, message)
    }
}

fn lock(m: &Mutex<HashMap<String, u32>>) -> std::sync::MutexGuard<'_, HashMap<String, u32>> {
    // a test that panicked holding the lock leaves the counts as they were
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn set(m: &Mutex<HashMap<String, u32>>, message: &str, count: u32) {
    let mut m = lock(m);
    if count == 0 {
        m.remove(message);
    } else {
        m.insert(message.to_owned(), count);
    }
}

fn take(m: &Mutex<HashMap<String, u32>>, message: &str) -> bool {
    let mut m = lock(m);
    match m.get_mut(message) {
        Some(n) if *n > 0 => {
            *n -= 1;
            if *n == 0 {
                m.remove(message);
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_as_many_as_asked_then_none() {
        let f = Faults::default();
        f.drop_events("CrossedRegion", 2);
        f.drop_messages("AgentMovementComplete", 1);
        assert_eq!(
            f.pending(),
            vec![
                ("AgentMovementComplete".into(), 1),
                ("CrossedRegion".into(), 2)
            ]
        );
        assert!(!f.take_event("TeleportFinish"));
        assert!(f.take_event("CrossedRegion"));
        assert!(f.take_event("CrossedRegion"));
        assert!(!f.take_event("CrossedRegion"));
        // events and messages are counted apart
        assert!(!f.take_event("AgentMovementComplete"));
        assert!(f.take_message("AgentMovementComplete"));
        assert!(f.pending().is_empty());
    }

    #[test]
    fn zero_takes_a_drop_back() {
        let f = Faults::default();
        f.drop_events("CrossedRegion", 3);
        f.drop_events("CrossedRegion", 0);
        assert!(!f.take_event("CrossedRegion"));
    }
}
