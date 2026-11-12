//! Logs the local chat and the IMs the bot receives.

use ferro_client::events::{chat_type, dialog};
use ferro_client::{Client, Event};
use tokio::sync::broadcast::error::RecvError;

pub async fn run(client: Client) {
    let mut events = client.subscribe();
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
