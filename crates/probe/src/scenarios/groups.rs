//! Groups, as one resident uses them: the bot makes a group, finds its default roles and itself
//! the owner, picks a title, makes the group active, talks in its chat, sends a notice, leaves it
//! and joins it again. These are the behaviours a rebuild of the grid's groups system has to keep.

use super::Probe;
use ferro_client::groups::NewGroup;
use ferro_client::{Client, Event};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// GP_MEMBER_EJECT (llgroupmgr.h): an owner has it, a plain member does not.
const POWER_EJECT: u64 = 1 << 2;
/// IM_GROUP_NOTICE.
const IM_GROUP_NOTICE: u8 = 32;
/// How long the region may take to answer a group request or to deliver a notice.
const ANSWER_WAIT: Duration = Duration::from_secs(15);

impl Probe {
    pub(super) async fn groups(&mut self, client: &Client) {
        let me = client.state().agent_id;
        let name = format!("Probe group {} {:04x}", self.round, rand_suffix());

        // made, with the bot as founder and owner
        let t = Instant::now();
        let group = match client
            .create_group(NewGroup {
                name: name.clone(),
                charter: "Made by ferro-probe.".into(),
                show_in_list: true,
                insignia: Uuid::nil(),
                membership_fee: 0,
                open_enrollment: true,
                allow_publish: false,
                mature_publish: false,
            })
            .await
        {
            Ok(c) if c.success && !c.id.is_nil() => {
                self.emit(client, "groups-create", true, t, String::new());
                c.id
            }
            Ok(c) => {
                self.emit(
                    client,
                    "groups-create",
                    false,
                    t,
                    format!("refused: {}", c.message),
                );
                return;
            }
            Err(e) => {
                self.emit(client, "groups-create", false, t, e.to_string());
                return;
            }
        };

        // the group list comes over the event queue (AgentGroupDataUpdate), after the reply
        let t = Instant::now();
        wait_until(ANSWER_WAIT, || {
            client.state().groups.iter().any(|g| g.id == group)
        })
        .await;
        let mine = client
            .state()
            .groups
            .iter()
            .find(|g| g.id == group)
            .cloned();
        let (ok, note) = match &mine {
            None => (false, "not in the bot's group list".to_owned()),
            Some(g) if g.powers & POWER_EJECT == 0 => {
                (false, format!("powers {:#x} lack Eject", g.powers))
            }
            Some(_) => (true, String::new()),
        };
        self.emit(client, "groups-in-my-list", ok, t, note);

        // the three default roles, and the founder an owner in the member list
        let t = Instant::now();
        let roles = client.group_roles(group).await;
        let (ok, note) = match &roles {
            Ok(r) => {
                let mut names: Vec<&str> = r.iter().map(|x| x.name.as_str()).collect();
                names.sort_unstable();
                (
                    names == ["Everyone", "Officers", "Owners"],
                    names.join(", "),
                )
            }
            Err(e) => (false, e.to_string()),
        };
        self.emit(client, "groups-default-roles", ok, t, note);

        let t = Instant::now();
        let (ok, note) = match client.group_members(group).await {
            Ok(m) => match m.iter().find(|x| x.id == me) {
                Some(x) if x.is_owner && m.len() == 1 => (true, String::new()),
                Some(x) => (false, format!("{} members, owner {}", m.len(), x.is_owner)),
                None => (
                    false,
                    format!("{} members, the founder not among them", m.len()),
                ),
            },
            Err(e) => (false, e.to_string()),
        };
        self.emit(client, "groups-founder-owner", ok, t, note);

        // a title chosen, then the group active
        let t = Instant::now();
        let everyone = Uuid::nil();
        let titled = match client.group_activate_title(group, everyone).await {
            Ok(()) => match client.group_titles(group).await {
                Ok(titles) => match titles.iter().find(|x| x.selected) {
                    Some(x) if x.role == everyone => (true, x.title.clone()),
                    Some(x) => (false, format!("selected title is {:?}", x.title)),
                    None => (false, "no title selected".into()),
                },
                Err(e) => (false, e.to_string()),
            },
            Err(e) => (false, e.to_string()),
        };
        self.emit(client, "groups-title", titled.0, t, titled.1);

        let t = Instant::now();
        let ok = client.group_activate(group).await.is_ok()
            && wait_until(ANSWER_WAIT, || {
                client
                    .state()
                    .active_group
                    .as_ref()
                    .is_some_and(|a| a.id == group)
            })
            .await;
        self.emit(client, "groups-activate", ok, t, String::new());

        // its chat: the bot joins the session and hears its own line
        let mut events = client.subscribe();
        let t = Instant::now();
        let line = format!("probe line {}", self.round);
        let heard = match client.group_chat_join(group).await {
            Ok(()) => {
                let _ = client.group_chat_send(group, &line).await;
                next_event(&mut events, ANSWER_WAIT, |e| {
                    matches!(e, Event::GroupChat { group: g, message, .. } if *g == group && *message == line)
                })
                .await
            }
            Err(_) => false,
        };
        self.emit(
            client,
            "groups-chat",
            heard,
            t,
            if heard {
                String::new()
            } else {
                "the line did not come back".into()
            },
        );

        // a notice, received by the bot as a member who accepts notices
        let mut events = client.subscribe();
        let t = Instant::now();
        let subject = format!("Probe notice {}", self.round);
        let got = match client.group_notice(group, &subject, "body", None).await {
            Ok(()) => {
                next_event(&mut events, ANSWER_WAIT, |e| {
                    matches!(e, Event::InstantMessage(im) if im.dialog == IM_GROUP_NOTICE && im.message.starts_with(&subject))
                })
                .await
            }
            Err(_) => false,
        };
        self.emit(
            client,
            "groups-notice",
            got,
            t,
            if got {
                String::new()
            } else {
                "no notice came".into()
            },
        );

        // left, then joined again: the group is open
        let t = Instant::now();
        let left = client.group_leave(group).await.unwrap_or(false)
            && wait_until(ANSWER_WAIT, || {
                !client.state().groups.iter().any(|g| g.id == group)
            })
            .await;
        self.emit(client, "groups-leave", left, t, String::new());

        let t = Instant::now();
        let joined = client.group_join(group).await.unwrap_or(false)
            && wait_until(ANSWER_WAIT, || {
                client.state().groups.iter().any(|g| g.id == group)
            })
            .await;
        let note = match client.group_members(group).await {
            Ok(m) => format!("{} members", m.len()),
            Err(e) => e.to_string(),
        };
        self.emit(client, "groups-join", joined, t, note);
    }
}

fn rand_suffix() -> u16 {
    (Uuid::new_v4().as_u128() & 0xffff) as u16
}

async fn wait_until(wait: Duration, done: impl Fn() -> bool) -> bool {
    let t = Instant::now();
    while t.elapsed() < wait {
        if done() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    done()
}

async fn next_event(
    events: &mut tokio::sync::broadcast::Receiver<Event>,
    wait: Duration,
    matches: impl Fn(&Event) -> bool,
) -> bool {
    tokio::time::timeout(wait, async {
        loop {
            match events.recv().await {
                Ok(e) if matches(&e) => return true,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                _ => {}
            }
        }
    })
    .await
    .unwrap_or(false)
}
