//! Group invitations: accepted from the masters, as SecondBot's InteractionService does
//! (BotGroupInviteOffer, its GroupInviteLevel "Owner"), left unanswered from anyone else, as
//! SecondBot leaves a rejected one.
//!
//! A deliberate difference: an invitation with a membership fee is never accepted, from a master
//! either, so that the bot cannot be made to pay to join a group.

use crate::services::lures::is_master;
use ferro_client::events::dialog;
use ferro_client::{Client, Event, InstantMessage};
use tokio::sync::broadcast::error::RecvError;

/// The membership fee of an invitation (invite_bucket_t: a big-endian S32 then the role); `None`
/// when the bucket is not one.
fn fee(bucket: &[u8]) -> Option<i32> {
    let bytes: [u8; 4] = bucket.get(..4)?.try_into().ok()?;
    (bucket.len() == 20).then(|| i32::from_be_bytes(bytes))
}

async fn on_invitation(client: &Client, masters: &[String], im: InstantMessage) {
    if !is_master(masters, &im.from_name) {
        log::info!(
            "group invitation from {} left unanswered: not a master",
            im.from_name
        );
        return;
    }
    match fee(&im.binary_bucket) {
        Some(0) | None => {}
        Some(fee) => {
            log::warn!(
                "group invitation from {} with a fee of L${fee} left unanswered: the bot does not pay to join",
                im.from_name
            );
            return;
        }
    }
    // the invitation comes from the group: its id is the sender's
    match client
        .answer_group_invite(im.from_agent_id, im.id, true)
        .await
    {
        Ok(()) => log::info!("group invitation from {} accepted", im.from_name),
        Err(e) => log::warn!("group invitation from {}: {e}", im.from_name),
    }
}

pub async fn run(client: Client, masters: Vec<String>) {
    let mut events = client.subscribe();
    loop {
        match events.recv().await {
            Ok(Event::InstantMessage(im)) if im.dialog == dialog::GROUP_INVITATION => {
                let (client, masters) = (client.clone(), masters.clone());
                tokio::spawn(async move { on_invitation(&client, &masters, im).await });
            }
            Ok(_) => {}
            Err(RecvError::Lagged(n)) => log::warn!("group invitations: {n} events missed"),
            Err(RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fee_of_an_invitation() {
        let mut bucket = 50i32.to_be_bytes().to_vec();
        bucket.extend_from_slice(&[0; 16]);
        assert_eq!(fee(&bucket), Some(50));
        assert_eq!(fee(&[0; 20]), Some(0));
        assert_eq!(fee(&[0]), None);
    }
}
