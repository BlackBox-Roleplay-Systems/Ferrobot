//! Scripts asking for permissions (ScriptQuestion, llRequestPermissions): an object of an avatar
//! on the animators list (AddToAllowAnimations) gets PERMISSION_TRIGGER_ANIMATION, and only that;
//! any other question is left unanswered, which is a no.

use crate::animators::Animators;
use ferro_client::events::ScriptQuestion;
use ferro_client::{Client, Event};
use std::sync::Arc;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

/// SCRIPT_PERMISSION_TRIGGER_ANIMATION (lscript_byteformat.h)
const TRIGGER_ANIMATION: i32 = 0x10;

/// "First Resident" is shown as "First" (LLCacheName::buildLegacyName); names compared so.
fn legacy(name: &str) -> String {
    let n = name.trim().to_ascii_lowercase();
    n.strip_suffix(" resident").map_or(n.clone(), str::to_owned)
}

async fn on_question(client: Client, animators: Arc<Animators>, q: ScriptQuestion) {
    if q.questions & TRIGGER_ANIMATION == 0 {
        return;
    }
    let allowed = animators.list();
    if allowed.is_empty() {
        return;
    }
    let names = client.names(&allowed).await.unwrap_or_default();
    let owner = legacy(&q.owner_name);
    if !names.values().any(|n| legacy(n) == owner) {
        log::info!("{} asks to animate the bot: not allowed", q.object_name);
        return;
    }
    log::info!("{} may animate the bot", q.object_name);
    if let Err(e) = client
        .answer_script(q.task, q.item, TRIGGER_ANIMATION)
        .await
    {
        log::warn!("script permissions: {e}");
    }
}

pub async fn run(client: Client, mut events: Receiver<Event>, animators: Arc<Animators>) {
    loop {
        match events.recv().await {
            Ok(Event::ScriptQuestion(q)) => {
                tokio::spawn(on_question(client.clone(), animators.clone(), q));
            }
            Ok(_) => {}
            Err(RecvError::Lagged(n)) => log::warn!("script permissions: {n} events missed"),
            Err(RecvError::Closed) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_with_and_without_resident() {
        assert_eq!(legacy("Some Resident"), "some");
        assert_eq!(legacy("Other Avatar"), "other avatar");
        assert_eq!(legacy(" some "), "some");
    }
}
