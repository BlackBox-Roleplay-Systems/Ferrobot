//! What the bot heard, as SecondBot's DataStoreService keeps it for its Chat and Groupchat
//! commands: the last local chat lines ("{Av} Name: text", "{Obj} …", "{Sys} …", the bot's own
//! "{BOT} Name: text"), and a window per IM partner and per group ("Name: text"), each with an
//! unread flag that reading the window clears. The limits are SecondBot's defaults.
//!
//! Windows are kept by their owner (the avatar, the group), which is what the commands ask by;
//! SecondBot keys them by IM session and maps sessions to owners.

use ferro_client::events::{chat_type, dialog};
use ferro_client::{Client, Event};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

/// LocalChatHistoryLimit, ImChatHistoryLimit, GroupChatHistoryLimitPerGroup (DataStoreConfig).
const LOCAL_LIMIT: usize = 150;
const IM_LIMIT: usize = 50;
const GROUP_LIMIT: usize = 50;

/// Names SecondBot never records ("hard_blocked_agents").
const BLOCKED: [&str; 2] = ["secondlife", "second life"];

struct Window {
    group: bool,
    /// The avatar's name, for an IM window.
    name: String,
    lines: VecDeque<String>,
    unread: bool,
}

#[derive(Default)]
struct Inner {
    local: VecDeque<String>,
    windows: HashMap<Uuid, Window>,
}

#[derive(Default)]
pub struct History {
    inner: Mutex<Inner>,
}

fn push(lines: &mut VecDeque<String>, line: String, limit: usize) {
    lines.push_back(line);
    while lines.len() > limit {
        lines.pop_front();
    }
}

impl History {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn local_line(&self, line: String) {
        push(&mut self.lock().local, line, LOCAL_LIMIT);
    }

    /// Open a window (OpenChatWindow) if it is not.
    pub fn open(&self, owner: Uuid, group: bool, name: &str) {
        let mut inner = self.lock();
        let w = inner.windows.entry(owner).or_insert_with(|| Window {
            group,
            name: name.to_owned(),
            lines: VecDeque::new(),
            unread: false,
        });
        if !name.is_empty() && !group {
            w.name = name.to_owned();
        }
    }

    /// A line in a window (RecordChat): marks it unread unless it is the bot's own.
    pub fn record(&self, owner: Uuid, from: &str, message: &str, own: bool) {
        let mut inner = self.lock();
        if let Some(w) = inner.windows.get_mut(&owner) {
            let limit = if w.group { GROUP_LIMIT } else { IM_LIMIT };
            push(&mut w.lines, format!("{from}: {message}"), limit);
            if !own {
                w.unread = true;
            }
        }
    }

    pub fn local(&self) -> Vec<String> {
        self.lock().local.iter().cloned().collect()
    }

    /// chatwindows: `{avatar: name}` of the IM windows.
    pub fn im_windows(&self) -> BTreeMap<String, String> {
        self.lock()
            .windows
            .iter()
            .filter(|(_, w)| !w.group)
            .map(|(id, w)| (id.to_string(), w.name.clone()))
            .collect()
    }

    fn unread(&self, group: bool) -> Vec<Uuid> {
        let mut v: Vec<Uuid> = self
            .lock()
            .windows
            .iter()
            .filter(|(_, w)| w.group == group && w.unread)
            .map(|(id, _)| *id)
            .collect();
        v.sort();
        v
    }

    pub fn im_unread(&self) -> Vec<Uuid> {
        self.unread(false)
    }

    pub fn group_unread(&self) -> Vec<Uuid> {
        self.unread(true)
    }

    pub fn group_has_unread(&self, group: Uuid) -> bool {
        self.lock()
            .windows
            .get(&group)
            .is_some_and(|w| w.group && w.unread)
    }

    /// A window's lines; reading marks it read. Empty for an unknown window, or one of the other
    /// kind.
    pub fn read(&self, owner: Uuid, group: bool) -> Vec<String> {
        let mut inner = self.lock();
        match inner.windows.get_mut(&owner) {
            Some(w) if w.group == group => {
                w.unread = false;
                w.lines.iter().cloned().collect()
            }
            _ => Vec::new(),
        }
    }

    /// GroupchatClearAll: the group windows emptied.
    pub fn clear_groups(&self) {
        for w in self.lock().windows.values_mut().filter(|w| w.group) {
            w.lines.clear();
            w.unread = false;
        }
    }
}

/// The group of a group notice: its bucket's notice_bucket_header_t (has_inventory, asset type,
/// then the group's id), else the IM's own id.
fn notice_group(bucket: &[u8], id: Uuid) -> Uuid {
    bucket
        .get(2..18)
        .and_then(|b| Uuid::from_slice(b).ok())
        .filter(|g| !g.is_nil())
        .unwrap_or(id)
}

/// Fill the history from the client's events (DataStoreService's ChatFromSim and ImEvent).
pub async fn run(client: Client, history: Arc<History>) {
    let mut events = client.subscribe();
    loop {
        match events.recv().await {
            Ok(Event::Chat(c)) => {
                let spoken = matches!(
                    c.chat_type,
                    chat_type::WHISPER | chat_type::SAY | chat_type::SHOUT
                );
                if !spoken
                    || c.source_id == client.state().agent_id
                    || BLOCKED.contains(&c.from_name.to_lowercase().as_str())
                {
                    continue;
                }
                let source = match c.source_type {
                    0 => "{Sys}",
                    2 => "{Obj}",
                    _ => "{Av}",
                };
                history.local_line(format!("{source} {}: {}", c.from_name, c.message));
            }
            Ok(Event::InstantMessage(im)) => match im.dialog {
                dialog::NOTHING_SPECIAL if im.from_agent_id != client.state().agent_id => {
                    history.open(im.from_agent_id, false, &im.from_name);
                    history.record(im.from_agent_id, &im.from_name, &im.message, false);
                }
                dialog::GROUP_NOTICE => {
                    let group = notice_group(&im.binary_bucket, im.id);
                    if client.state().groups.iter().any(|g| g.id == group) {
                        history.open(group, true, "");
                        history.record(group, "**Notice**", &im.message, false);
                    }
                }
                _ => {}
            },
            Ok(Event::GroupChat {
                group,
                from_id,
                from_name,
                message,
            }) => {
                history.open(group, true, "");
                let own = from_id == client.state().agent_id;
                history.record(group, &from_name, &message, own);
            }
            Ok(_) => {}
            Err(RecvError::Lagged(n)) => log::warn!("history: {n} events missed"),
            Err(RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_keep_their_lines_and_reading_marks_them_read() {
        let h = History::default();
        let av = Uuid::from_u128(1);
        let group = Uuid::from_u128(2);
        h.record(av, "Nobody", "dropped: no window", false);
        h.open(av, false, "Some Resident");
        h.record(av, "Some Resident", "hi", false);
        h.record(av, "Bot", "hello", true);
        h.open(group, true, "");
        h.record(group, "Other Avatar", "group line", false);
        assert_eq!(h.im_unread(), vec![av]);
        assert_eq!(h.group_unread(), vec![group]);
        assert_eq!(h.im_windows()[&av.to_string()], "Some Resident");
        assert!(h.read(group, false).is_empty(), "not an IM window");
        assert_eq!(h.read(av, false), vec!["Some Resident: hi", "Bot: hello"]);
        assert!(h.im_unread().is_empty());
        assert!(h.group_has_unread(group));
        h.clear_groups();
        assert!(!h.group_has_unread(group));
        assert!(h.read(group, true).is_empty());
    }

    #[test]
    fn histories_keep_their_last_lines() {
        let h = History::default();
        for i in 0..200 {
            h.local_line(format!("{{Av}} A: {i}"));
        }
        let local = h.local();
        assert_eq!(local.len(), 150);
        assert_eq!(local[0], "{Av} A: 50");
    }

    #[test]
    fn a_notice_names_its_group_in_its_bucket() {
        let g = Uuid::from_u128(9);
        let mut bucket = vec![1, 0];
        bucket.extend_from_slice(g.as_bytes());
        bucket.extend_from_slice(b"item\0");
        assert_eq!(notice_group(&bucket, Uuid::from_u128(5)), g);
        assert_eq!(notice_group(&[0], Uuid::from_u128(5)), Uuid::from_u128(5));
    }
}
