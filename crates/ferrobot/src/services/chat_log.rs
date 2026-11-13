//! Logs the local chat, the IMs and the group chat the bot receives.

use ferro_client::events::{chat_type, dialog};
use ferro_client::{Client, Event};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

pub async fn run(client: Client, mut events: Receiver<Event>) {
    loop {
        match events.recv().await {
            Ok(Event::Chat(c)) => {
                let spoken = matches!(
                    c.chat_type,
                    chat_type::WHISPER | chat_type::SAY | chat_type::SHOUT
                );
                if spoken && c.source_id != client.state().agent_id {
                    log::info!("[chat] {}: {}", c.from_name, c.message);
                }
            }
            Ok(Event::GroupChat {
                group,
                from_name,
                message,
                ..
            }) => {
                let name = client
                    .state()
                    .groups
                    .iter()
                    .find(|g| g.id == group)
                    .map_or_else(|| group.to_string(), |g| g.name.clone());
                log::info!("[group {name}] {from_name}: {message}");
            }
            Ok(Event::InstantMessage(im)) if im.dialog == dialog::SESSION_SEND => {}
            Ok(Event::InstantMessage(im)) => {
                if im.dialog == dialog::NOTHING_SPECIAL {
                    log::info!("[im] {}: {}", im.from_name, im.message);
                } else {
                    log::debug!(
                        "instant message, dialog {} from {}",
                        im.dialog,
                        im.from_name
                    );
                }
            }
            Ok(_) => {}
            Err(RecvError::Lagged(n)) => log::warn!("chat log: {n} events missed"),
            Err(RecvError::Closed) => return,
        }
    }
}
