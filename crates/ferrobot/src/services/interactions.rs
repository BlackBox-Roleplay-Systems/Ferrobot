//! Offers to the bot, answered by SecondBot's interaction rules (InteractionService.ProcessRequest):
//! for each kind, a switch (accept at all) and a level: "Owner" (the masters, the default),
//! "Friends" (the bot's friends; not for friendship offers), or "Anyone".
//!
//! - teleport offers (IM_LURE_USER): accepted, or ignored (BotTeleportOffer);
//! - teleport requests (IM_TELEPORT_REQUEST): the bot offers a teleport back, or declines;
//! - group invitations (IM_GROUP_INVITATION): accepted, or left unanswered (BotGroupInviteOffer);
//! - inventory offers (IM_INVENTORY_OFFERED, IM_TASK_INVENTORY_OFFERED): accepted into the
//!   inventory's root folder, or declined (BotInventoryOffer);
//! - friendship offers (IM_FRIENDSHIP_OFFERED): accepted, or ignored (BotFriendRequested).
//!
//! Deliberate differences: masters are recognised by the sender's key where the grid gives it
//! (masters.rs); a group invitation with a membership fee is never accepted; SecondBot's one-time
//! "next accept" list and its JSON event output are not here yet (TASKS.md).

use crate::masters::Masters;
use crate::services::homebound::Homebound;
use ferro_client::events::dialog;
use ferro_client::{Client, Event, InstantMessage};
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Owner,
    Friends,
    Anyone,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        match s.trim().to_ascii_lowercase().as_str() {
            "owner" => Some(Level::Owner),
            "friends" => Some(Level::Friends),
            "anyone" => Some(Level::Anyone),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rule {
    pub enabled: bool,
    pub level: Level,
}

/// SecondBot's InteractionConfig: every kind on, at level "Owner", by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    pub teleports: Rule,
    pub group_invites: Rule,
    pub inventory: Rule,
    pub friendships: Rule,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Teleport,
    GroupInvite,
    Inventory,
    Friendship,
}

/// ProcessRequest: whether the sender may have this, and why not.
async fn allowed(
    client: &Client,
    masters: &Masters,
    rule: Rule,
    kind: Kind,
    im: &InstantMessage,
) -> Result<(), &'static str> {
    if !rule.enabled {
        return Err("disabled by the settings");
    }
    // a group invitation comes from the group: only the inviter's name tells who sent it
    let master = if kind == Kind::GroupInvite {
        masters.by_name(&im.from_name)
    } else {
        masters.by_key(client, im.from_agent_id).await
    };
    if master {
        return Ok(());
    }
    match rule.level {
        Level::Anyone => Ok(()),
        Level::Friends
            if kind != Kind::Friendship && client.state().friends.contains(&im.from_agent_id) =>
        {
            Ok(())
        }
        Level::Friends => Err("not a friend"),
        Level::Owner => Err("not a master"),
    }
}

/// The membership fee of an invitation (invite_bucket_t: a big-endian S32 then the role); `None`
/// when the bucket is not one.
fn fee(bucket: &[u8]) -> Option<i32> {
    let bytes: [u8; 4] = bucket.get(..4)?.try_into().ok()?;
    (bucket.len() == 20).then(|| i32::from_be_bytes(bytes))
}

async fn on_offer(
    client: Client,
    masters: Arc<Masters>,
    rules: Rules,
    homebound: Arc<Homebound>,
    im: InstantMessage,
) {
    let (what, rule, kind) = match im.dialog {
        dialog::LURE_USER => ("teleport offer", rules.teleports, Kind::Teleport),
        dialog::TELEPORT_REQUEST => ("teleport request", rules.teleports, Kind::Teleport),
        dialog::GROUP_INVITATION => ("group invitation", rules.group_invites, Kind::GroupInvite),
        dialog::INVENTORY_OFFERED | dialog::TASK_INVENTORY_OFFERED => {
            ("inventory offer", rules.inventory, Kind::Inventory)
        }
        dialog::FRIENDSHIP_OFFERED => ("friendship offer", rules.friendships, Kind::Friendship),
        _ => return,
    };
    let from = &im.from_name;
    let verdict = match allowed(&client, &masters, rule, kind, &im).await {
        Err(why) => Err(why.to_owned()),
        Ok(()) if kind == Kind::GroupInvite => match fee(&im.binary_bucket) {
            Some(fee) if fee > 0 => Err(format!("a fee of L${fee}, the bot does not pay to join")),
            _ => Ok(()),
        },
        Ok(()) => Ok(()),
    };
    let from_task = im.dialog == dialog::TASK_INVENTORY_OFFERED;
    let done = match verdict {
        Err(why) => {
            log::info!("{what} from {from} refused: {why}");
            // the answers SecondBot gives to what it refuses
            match im.dialog {
                dialog::TELEPORT_REQUEST => {
                    client
                        .decline_teleport_request(im.from_agent_id, im.id)
                        .await
                }
                dialog::INVENTORY_OFFERED | dialog::TASK_INVENTORY_OFFERED => {
                    client
                        .answer_inventory_offer(im.from_agent_id, im.id, false, from_task)
                        .await
                }
                _ => Ok(()),
            }
        }
        Ok(()) => {
            log::info!("{what} from {from} accepted");
            match im.dialog {
                dialog::LURE_USER => {
                    // homebound does not bring the bot back from where it was sent
                    homebound.pause();
                    client.accept_lure(im.id).await.map(|region| {
                        log::info!("teleported to {region}");
                    })
                }
                dialog::TELEPORT_REQUEST => client.offer_teleport(im.from_agent_id, "").await,
                // the group's id is the invitation's sender
                dialog::GROUP_INVITATION => {
                    client
                        .answer_group_invite(im.from_agent_id, im.id, true)
                        .await
                }
                dialog::FRIENDSHIP_OFFERED => {
                    client.accept_friendship(im.from_agent_id, im.id).await
                }
                _ => {
                    client
                        .answer_inventory_offer(im.from_agent_id, im.id, true, from_task)
                        .await
                }
            }
        }
    };
    if let Err(e) = done {
        log::warn!("{what} from {from}: {e}");
    }
}

pub async fn run(client: Client, masters: Arc<Masters>, rules: Rules, homebound: Arc<Homebound>) {
    let mut events = client.subscribe();
    loop {
        match events.recv().await {
            Ok(Event::InstantMessage(im)) => {
                tokio::spawn(on_offer(
                    client.clone(),
                    masters.clone(),
                    rules,
                    homebound.clone(),
                    im,
                ));
            }
            Ok(_) => {}
            Err(RecvError::Lagged(n)) => log::warn!("interactions: {n} events missed"),
            Err(RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_as_secondbot_names_them() {
        assert_eq!(Level::parse("Owner"), Some(Level::Owner));
        assert_eq!(Level::parse("anyone"), Some(Level::Anyone));
        assert_eq!(Level::parse(" Friends "), Some(Level::Friends));
        assert_eq!(Level::parse("everyone"), None);
    }

    #[test]
    fn the_fee_of_an_invitation() {
        let mut bucket = 50i32.to_be_bytes().to_vec();
        bucket.extend_from_slice(&[0; 16]);
        assert_eq!(fee(&bucket), Some(50));
        assert_eq!(fee(&[0; 20]), Some(0));
        assert_eq!(fee(&[0]), None);
    }
}
