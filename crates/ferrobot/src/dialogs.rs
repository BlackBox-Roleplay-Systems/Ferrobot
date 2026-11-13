//! Script dialogs, as SecondBot's DialogService keeps and relays them: each dialog gets a number
//! (1 to 999, then 1 again) and lives 440 s; it is relayed as JSON `{message, buttons,
//! objectname, dialogid, expires}` to the relay targets: chat channels, avatars by IM, and HTTP
//! addresses (a POST to `<url>/Dialog/Relay`, signed with the shared secret). DialogResponce
//! presses a button. Optionally, only the dialogs of one owner's objects are kept.
//!
//! A deliberate difference: SecondBot's clean-up drops the dialogs that have not expired yet;
//! here the expired ones go.

use crate::api::signing::sign_with;
use ferro_client::events::chat_type;
use ferro_client::{Client, Event, ScriptDialog};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

/// How long a dialog can be answered (DialogService: 440 s).
const LIFETIME: i64 = 440;

/// The JSON a dialog is relayed as (DialogWindow, its fields in their order).
#[derive(Serialize)]
struct Window<'a> {
    message: &'a str,
    buttons: &'a [String],
    objectname: &'a str,
    dialogid: i32,
    expires: i64,
}

#[derive(Default)]
struct Inner {
    windows: HashMap<i32, (ScriptDialog, i64)>,
    next: i32,
    channels: Vec<i32>,
    avatars: Vec<Uuid>,
    urls: Vec<String>,
    owner_only: Option<Uuid>,
}

#[derive(Default)]
pub struct Dialogs {
    inner: Mutex<Inner>,
}

/// The relay targets from the settings.
#[derive(Debug, Clone, Default)]
pub struct Targets {
    pub channel: Option<i32>,
    pub avatar: Option<Uuid>,
    pub url: Option<String>,
    pub owner_only: Option<Uuid>,
}

fn toggle<T: PartialEq>(list: &mut Vec<T>, item: T) -> &'static str {
    match list.iter().position(|x| *x == item) {
        Some(i) => {
            list.remove(i);
            "removed"
        }
        None => {
            list.push(item);
            "added"
        }
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

impl Dialogs {
    pub fn new(t: Targets) -> Self {
        let d = Dialogs::default();
        {
            let mut i = d.lock();
            i.next = 1;
            i.channels.extend(t.channel.filter(|c| *c >= 0));
            i.avatars.extend(t.avatar.filter(|a| !a.is_nil()));
            i.urls.extend(t.url.filter(|u| u.starts_with("http")));
            i.owner_only = t.owner_only.filter(|o| !o.is_nil());
        }
        d
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn toggle_channel(&self, channel: i32) -> &'static str {
        toggle(&mut self.lock().channels, channel)
    }

    pub fn toggle_avatar(&self, avatar: Uuid) -> &'static str {
        toggle(&mut self.lock().avatars, avatar)
    }

    pub fn toggle_url(&self, url: &str) -> &'static str {
        toggle(&mut self.lock().urls, url.to_owned())
    }

    /// Keep a dialog: its number and the JSON to relay, or `None` when its owner is not the one
    /// asked for.
    fn add(&self, d: ScriptDialog, now: i64) -> Option<(i32, String)> {
        let mut i = self.lock();
        if i.owner_only.is_some_and(|o| o != d.owner_id) {
            return None;
        }
        i.windows.retain(|_, (_, expires)| *expires > now);
        let id = i.next.max(1);
        i.next = if id >= 999 { 1 } else { id + 1 };
        let expires = now + LIFETIME;
        let json = serde_json::to_string(&Window {
            message: &d.message,
            buttons: &d.buttons,
            objectname: &d.object_name,
            dialogid: id,
            expires,
        })
        .unwrap_or_default();
        i.windows.insert(id, (d, expires));
        Some((id, json))
    }

    /// DialogAction: the dialog and the button to press, or SecondBot's answer why not.
    pub fn take(&self, id: i32, button: &str) -> Result<(ScriptDialog, i32), &'static str> {
        self.take_at(id, button, now())
    }

    fn take_at(
        &self,
        id: i32,
        button: &str,
        now: i64,
    ) -> Result<(ScriptDialog, i32), &'static str> {
        let mut i = self.lock();
        i.windows.retain(|_, (_, expires)| *expires > now);
        let Some((d, _)) = i.windows.get(&id) else {
            return Err("Invaild dialog window");
        };
        let Some(index) = d.buttons.iter().position(|b| b == button) else {
            return Err("Invaild dialog button");
        };
        let index = i32::try_from(index).unwrap_or(0);
        let (d, _) = i.windows.remove(&id).ok_or("Invaild dialog window")?;
        Ok((d, index))
    }

    fn targets(&self) -> (Vec<i32>, Vec<Uuid>, Vec<String>) {
        let i = self.lock();
        (i.channels.clone(), i.avatars.clone(), i.urls.clone())
    }
}

fn form_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_owned(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Keep each dialog and relay it.
pub async fn run(client: Client, dialogs: Arc<Dialogs>, secret: Option<String>) {
    let http = reqwest::Client::new();
    let mut events = client.subscribe();
    loop {
        match events.recv().await {
            Ok(Event::ScriptDialog(d)) => {
                let from = d.object_name.clone();
                let Some((id, json)) = dialogs.add(d, now()) else {
                    continue;
                };
                log::info!("script dialog {id} from {from}");
                let (channels, avatars, urls) = dialogs.targets();
                for ch in channels {
                    let _ = client.chat(&json, ch, chat_type::SAY).await;
                }
                for av in avatars {
                    let _ = client.im(av, &json).await;
                }
                let st = client.state();
                for url in urls {
                    let unixtime = now();
                    // sha1(unixtime + "DialogRelay" + the bot's key + the event + the secret)
                    let token = sign_with(
                        &format!("{unixtime}DialogRelay{}{json}", st.agent_id),
                        &[],
                        None,
                        secret.as_deref().unwrap_or(""),
                    );
                    let body = format!(
                        "token={}&unixtime={unixtime}&method=Dialog&action=Relay&botname={}&event={}",
                        form_encode(&token),
                        form_encode(&st.name),
                        form_encode(&json)
                    );
                    let target = format!("{}/Dialog/Relay", url.trim_end_matches('/'));
                    let sent = http
                        .post(&target)
                        .header("Content-Type", "application/x-www-form-urlencoded")
                        .timeout(std::time::Duration::from_secs(30))
                        .body(body)
                        .send()
                        .await;
                    if let Err(e) = sent {
                        log::warn!("dialog relay by HTTP failed: {}", e.without_url());
                    }
                }
            }
            Ok(_) => {}
            Err(RecvError::Lagged(n)) => log::warn!("dialogs: {n} events missed"),
            Err(RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(owner: u128) -> ScriptDialog {
        ScriptDialog {
            object_id: Uuid::from_u128(5),
            object_name: "Door".into(),
            owner_id: Uuid::from_u128(owner),
            owner_name: "Some Resident".into(),
            message: "Open?".into(),
            channel: -99,
            buttons: vec!["Yes".into(), "No".into()],
        }
    }

    #[test]
    fn dialogs_are_numbered_relayed_and_answered() {
        let d = Dialogs::new(Targets::default());
        let (id, json) = d.add(dialog(1), 1000).expect("kept");
        assert_eq!(id, 1);
        assert_eq!(
            json,
            r#"{"message":"Open?","buttons":["Yes","No"],"objectname":"Door","dialogid":1,"expires":1440}"#
        );
        assert_eq!(d.add(dialog(1), 1000).expect("kept").0, 2);
        assert_eq!(
            d.take_at(9, "Yes", 1000).err(),
            Some("Invaild dialog window")
        );
        assert_eq!(
            d.take_at(1, "Maybe", 1000).err(),
            Some("Invaild dialog button")
        );
        assert_eq!(d.take_at(1, "No", 1000).map(|(_, i)| i), Ok(1));
        assert_eq!(
            d.take_at(1, "No", 1000).err(),
            Some("Invaild dialog window")
        );
        assert_eq!(
            d.take_at(2, "Yes", 1441).err(),
            Some("Invaild dialog window"),
            "expired"
        );
    }

    #[test]
    fn the_numbers_wrap_and_old_dialogs_expire() {
        let d = Dialogs::new(Targets::default());
        d.lock().next = 999;
        assert_eq!(d.add(dialog(1), 0).expect("kept").0, 999);
        assert_eq!(d.add(dialog(1), 0).expect("kept").0, 1);
        d.add(dialog(1), 1000);
        assert!(!d.lock().windows.contains_key(&999), "expired");
    }

    #[test]
    fn owner_only_and_relay_targets() {
        let d = Dialogs::new(Targets {
            owner_only: Some(Uuid::from_u128(1)),
            channel: Some(-1),
            ..Targets::default()
        });
        assert!(d.add(dialog(2), 0).is_none());
        assert!(d.add(dialog(1), 0).is_some());
        assert!(d.targets().0.is_empty(), "a negative channel is no target");
        assert_eq!(d.toggle_channel(5), "added");
        assert_eq!(d.toggle_channel(5), "removed");
        assert_eq!(d.toggle_url("http://x"), "added");
    }
}
