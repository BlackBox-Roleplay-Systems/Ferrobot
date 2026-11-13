//! RLV, when FERRO_RLV is on (SecondBot's RLVService, rlv_Enabled): what objects say to the bot
//! with llOwnerSay as "@" commands (crate::rlv reads them), and what they ask done: answers on a
//! channel, sitting and standing (the Sit and Stand commands, as SecondBot runs them), turning,
//! teleports, and the outfit from the shared folder #RLV (crate::rlv::folders). An attachment
//! that keeps itself on (@detach=n) is never taken off by RLV's own detaches (SecondBot's
//! AllowDetach); the bot's commands still take it off, as SecondBot's do.
//!
//! Deliberate difference: only an object owned by the bot or by a master drives it (the line's
//! owner, ChatFromSimulator's OwnerID). SecondBot takes any OwnerSay; the grid already sends one
//! only to the object's owner, so this is a second lock on the same door. The viewer's own checks
//! (Aurora's Rlva::receive) are followed too: an object's line, with a key, of 4096 bytes at most.

use crate::commands::{self, Ctx};
use crate::masters::Masters;
use crate::rlv::folders::Tree;
use crate::rlv::{self, Action, Rules};
use ferro_client::events::chat_type;
use ferro_client::inventory::InvItem;
use ferro_client::outfit::{BODYPART, CLOTHING, wearable_type};
use ferro_client::{ChatMessage, Client, ClientError, Event, Move};
use glam::Vec3;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

/// Lines waiting for their actions; more are dropped.
const QUEUE: usize = 64;
/// SecondBot's cleanup timer: the restrictions of objects gone are dropped this often.
const CLEANUP: Duration = Duration::from_secs(120);
/// EChatSourceType's CHAT_SOURCE_OBJECT.
const SOURCE_OBJECT: u8 = 2;

pub async fn run(
    ctx: Ctx,
    mut events: broadcast::Receiver<Event>,
    masters: Arc<Masters>,
    rules: Arc<Rules>,
) {
    log::info!("rlv on");
    let (tx, rx) = mpsc::channel(QUEUE);
    tokio::spawn(work(ctx.clone(), rules.clone(), rx));
    let mut cleanup = tokio::time::interval(CLEANUP);
    cleanup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // the first tick is at once
    cleanup.tick().await;
    loop {
        tokio::select! {
            e = events.recv() => match e {
                Ok(Event::Chat(c)) => {
                    let actions = heard(&ctx.client, &masters, &rules, &c).await;
                    if !actions.is_empty() && tx.try_send(actions).is_err() {
                        log::warn!("rlv: too many commands at once, {}'s dropped", c.from_name);
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    log::warn!("rlv: {n} events missed");
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = cleanup.tick() => clean(&ctx.client, &rules).await,
        }
    }
}

/// A line heard: an object's OwnerSay starting with "@", from the bot's own object or a
/// master's (SecondBot's BotChatEvents); its restrictions kept, its actions returned.
async fn heard(client: &Client, masters: &Masters, rules: &Rules, c: &ChatMessage) -> Vec<Action> {
    if c.chat_type != chat_type::OWNER_SAY || !c.message.starts_with('@') {
        return Vec::new();
    }
    if c.source_type != SOURCE_OBJECT || c.source_id.is_nil() || c.message.len() > rlv::MAX_LINE {
        return Vec::new();
    }
    let me = client.state().agent_id;
    if c.owner_id != me && !masters.by_key(client, c.owner_id).await {
        log::warn!(
            "rlv: {} is neither the bot's object nor a master's, ignored",
            c.from_name
        );
        return Vec::new();
    }
    rules.handle(c.source_id, &c.from_name, &c.message)
}

/// The actions, one line after the other, so that a slow one (an inventory fetch) does not hold
/// the events up.
async fn work(ctx: Ctx, rules: Arc<Rules>, mut lines: mpsc::Receiver<Vec<Action>>) {
    while let Some(actions) = lines.recv().await {
        for a in actions {
            log::debug!("rlv: {a:?}");
            if let Err(e) = act(&ctx, &rules, a).await {
                log::warn!("rlv: {e}");
            }
        }
    }
}

/// The restrictions of objects the region no longer shows dropped (SecondBot's
/// CleanupTimer_Elapsed); nothing while the bot cannot look.
async fn clean(client: &Client, rules: &Rules) {
    let senders = rules.senders();
    if senders.is_empty() || !client.state().online {
        return;
    }
    let mut present = HashMap::new();
    for id in senders {
        match client.find_object(id).await {
            Ok(found) => {
                present.insert(id, found.is_some());
            }
            Err(e) => {
                log::debug!("rlv: no cleanup ({e})");
                return;
            }
        }
    }
    rules.collect(&present);
}

/// SecondBot's Respond: normal chat on the channel.
async fn say(client: &Client, channel: i32, text: &str) -> Result<(), ClientError> {
    client.chat(text, channel, chat_type::SAY).await
}

/// The clothing and body parts worn (the Current Outfit Folder's).
async fn wearables(client: &Client) -> Result<Vec<InvItem>, ClientError> {
    Ok(client
        .outfit_items()
        .await?
        .into_iter()
        .filter(|i| matches!(i.asset_type, CLOTHING | BODYPART))
        .collect())
}

/// Take items off, those of an attachment that keeps itself on left (SecondBot's
/// RemoveFromOutfit and its AllowDetach).
async fn take_off(
    client: &Client,
    rules: &Rules,
    items: impl IntoIterator<Item = Uuid>,
) -> Result<(), ClientError> {
    let worn = client.worn().await?;
    let mut off = Vec::new();
    for id in items {
        if worn.iter().any(|w| w.item == id && rules.locked(w.object)) {
            log::info!("rlv: {id} keeps itself on (@detach=n)");
        } else if !off.contains(&id) {
            off.push(id);
        }
    }
    client.remove_from_outfit(&off).await
}

/// Run one of SecondBot's commands for RLV, as its RunCommand("RLV", …).
async fn command(ctx: &Ctx, name: &str, args: &[String]) {
    let (_, answer) = commands::run(ctx, name, args).await;
    log::info!("rlv: {name}: {answer}");
}

async fn act(ctx: &Ctx, rules: &Rules, a: Action) -> Result<(), ClientError> {
    let client = &ctx.client;
    match a {
        Action::Reply { channel, text } => say(client, channel, &text).await,
        Action::GetGroup { channel } => {
            // nothing while the bot has no active group (SecondBot's)
            let st = client.state();
            let name = st
                .active_group
                .as_ref()
                .filter(|g| !g.id.is_nil())
                .and_then(|g| st.groups.iter().find(|x| x.id == g.id))
                .map(|g| g.name.clone());
            match name {
                Some(name) => say(client, channel, &name).await,
                None => Ok(()),
            }
        }
        Action::SetGroup { name } => {
            let found: Vec<Uuid> = client
                .state()
                .groups
                .iter()
                .filter(|g| g.name.to_lowercase() == name.to_lowercase())
                .map(|g| g.id)
                .collect();
            for id in found {
                client.group_activate(id).await?;
            }
            Ok(())
        }
        Action::GetPath {
            channel,
            object,
            new,
        } => {
            let worn = client.worn().await?;
            let Some(w) = worn.iter().find(|w| w.object == object) else {
                // @getpath from what the bot does not wear: no answer; @getpathnew: an empty one
                return if new {
                    say(client, channel, "").await
                } else {
                    Ok(())
                };
            };
            let path = Tree::shared(client)
                .await?
                .and_then(|t| t.folder_of(w.item).map(|n| t.path(n)))
                .unwrap_or_default();
            say(client, channel, &path).await
        }
        Action::GetSitId { channel } => {
            let seat = client.state().sitting_on.unwrap_or_default();
            say(client, channel, &seat.to_string()).await
        }
        Action::Sit { target } => {
            command(ctx, "Sit", &[target.to_string()]).await;
            Ok(())
        }
        Action::Unsit => {
            command(ctx, "Stand", &[]).await;
            Ok(())
        }
        // LibreMetaverse's heading is from east, counter-clockwise: SecondBot gives it
        // PI/2 - rot; Ferrobot's is clockwise from north, as RLV's
        Action::SetRot { radians } => {
            client
                .move_bot(Move::Face(radians.to_degrees() as f32))
                .await
        }
        Action::TeleportGlobal { x, y, z } => {
            let Some((handle, lx, ly)) = rlv::global_to_region(x, y) else {
                return Ok(());
            };
            let client = client.clone();
            // SecondBot's RequestTeleport does not wait for the arrival
            tokio::spawn(async move {
                match client.teleport_to(handle, Vec3::new(lx, ly, z)).await {
                    Ok(_) => log::info!("rlv: teleported"),
                    Err(e) => log::warn!("rlv: teleport: {e}"),
                }
            });
            Ok(())
        }
        Action::TeleportRegion { region, x, y, z } => {
            let client = client.clone();
            tokio::spawn(async move {
                match client.teleport_to_region(&region, Vec3::new(x, y, z)).await {
                    Ok(_) => log::info!("rlv: teleported"),
                    Err(e) => log::warn!("rlv: teleport: {e}"),
                }
            });
            Ok(())
        }
        Action::GetOutfit { channel, layer } => {
            let types: HashSet<u32> = wearables(client).await?.iter().map(wearable_type).collect();
            say(client, channel, &rlv::outfit_status(&types, &layer)).await
        }
        Action::GetAttach { channel, point } => {
            let points: HashSet<u8> = client.worn().await?.iter().map(|w| w.point).collect();
            say(client, channel, &rlv::attach_status(&points, &point)).await
        }
        Action::Detach { what } => {
            let worn = client.worn().await?;
            if what.is_empty() {
                return take_off(client, rules, worn.iter().map(|w| w.item)).await;
            }
            if let Some(p) = rlv::point(&what) {
                // the first attachment at that point (SecondBot's)
                return match worn.iter().find(|w| w.point == p) {
                    Some(w) => take_off(client, rules, [w.item]).await,
                    None => Ok(()),
                };
            }
            let Some(tree) = Tree::shared(client).await? else {
                return Ok(());
            };
            match tree.find(&what) {
                Some(n) => {
                    take_off(
                        client,
                        rules,
                        tree.items(n, false, false).iter().map(|i| i.id),
                    )
                    .await
                }
                None => Ok(()),
            }
        }
        Action::DetachAll { folder } => {
            let Some(tree) = Tree::shared(client).await? else {
                return Ok(());
            };
            match tree.find(&folder) {
                Some(n) => {
                    take_off(
                        client,
                        rules,
                        tree.items(n, true, true).iter().map(|i| i.id),
                    )
                    .await
                }
                None => Ok(()),
            }
        }
        Action::DetachThis { object } => {
            let worn = client.worn().await?;
            let Some(w) = worn.iter().find(|w| w.object == object) else {
                return Ok(());
            };
            // its folder in #RLV, as the viewer: SecondBot takes any folder, the root of the
            // inventory too, and all under it
            let Some(tree) = Tree::shared(client).await? else {
                return Ok(());
            };
            match tree.folder_of(w.item) {
                Some(n) => {
                    take_off(
                        client,
                        rules,
                        tree.items(n, true, false).iter().map(|i| i.id),
                    )
                    .await
                }
                None => Ok(()),
            }
        }
        Action::RemOutfit { layer } => {
            let Some(t) = rlv::layer(&layer) else {
                return Ok(());
            };
            let items = wearables(client).await?;
            take_off(
                client,
                rules,
                items.iter().filter(|i| wearable_type(i) == t).map(|i| i.id),
            )
            .await
        }
        Action::Attach {
            folder,
            recursive,
            replace,
        } => {
            let Some(tree) = Tree::shared(client).await? else {
                return Ok(());
            };
            match tree.find(&folder) {
                Some(n) => {
                    client
                        .add_to_outfit(tree.items(n, recursive, false), replace)
                        .await
                }
                None => Ok(()),
            }
        }
        Action::GetInv { channel, folder } => {
            let names = Tree::shared(client)
                .await?
                .and_then(|t| t.find(&folder).map(|n| t.subfolders(n)))
                .unwrap_or_default();
            say(client, channel, &names).await
        }
        Action::GetInvWorn { channel, folder } => {
            let mut worn: HashSet<Uuid> = wearables(client).await?.iter().map(|i| i.id).collect();
            worn.extend(client.worn().await?.iter().map(|w| w.item));
            let status = Tree::shared(client)
                .await?
                .and_then(|t| t.find(&folder).map(|n| t.worn_status(n, &worn)))
                .unwrap_or_default();
            say(client, channel, &status).await
        }
        Action::FindFolder {
            channel,
            terms,
            all,
        } => {
            let found = Tree::shared(client)
                .await?
                .map(|t| t.find_words(&terms, all))
                .unwrap_or_default();
            say(client, channel, &found).await
        }
    }
}
