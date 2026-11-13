//! Teleport offers: the bot goes when a master offers (IM_LURE_USER), as SecondBot's
//! InteractionService accepts them from its masters list (BotTeleportOffer). Others are ignored, as
//! SecondBot does with a teleport offer it rejects. The master is recognised by key
//! (masters.rs). SecondBot's other levels (Anyone, Friends) and its AcceptTeleports switch come
//! with the interaction rules.

use crate::masters::Masters;
use ferro_client::events::dialog;
use ferro_client::{Client, Event};
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;

pub async fn run(client: Client, masters: Arc<Masters>) {
    let mut events = client.subscribe();
    loop {
        match events.recv().await {
            Ok(Event::InstantMessage(im)) if im.dialog == dialog::LURE_USER => {
                let (client, masters) = (client.clone(), masters.clone());
                tokio::spawn(async move {
                    if !masters.by_key(&client, im.from_agent_id).await {
                        log::info!("teleport offer from {} ignored: not a master", im.from_name);
                        return;
                    }
                    log::info!("teleport offer from {}, accepted", im.from_name);
                    match client.accept_lure(im.id).await {
                        Ok(region) => log::info!("teleported to {region}"),
                        Err(e) => log::warn!("teleport to {} failed: {e}", im.from_name),
                    }
                });
            }
            Ok(_) => {}
            Err(RecvError::Lagged(n)) => log::warn!("lures: {n} events missed"),
            Err(RecvError::Closed) => return,
        }
    }
}
