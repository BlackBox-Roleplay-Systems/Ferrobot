//! Scripts asking for permissions (ScriptQuestion, llRequestPermissions): an object of an avatar
//! granted animations (AddToAllowAnimations, or SetPermFlag's "animation", once or lasting) gets
//! PERMISSION_TRIGGER_ANIMATION, and only that; any other question is left unanswered, which is a
//! no. A one-time grant is used up by the first question it answers.

use crate::grants::{Grant, Grants};
use ferro_client::events::ScriptQuestion;
use ferro_client::{Client, Event};
use std::sync::Arc;
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

/// SCRIPT_PERMISSION_TRIGGER_ANIMATION (lscript_byteformat.h)
const TRIGGER_ANIMATION: i32 = 0x10;

async fn on_question(client: Client, grants: Arc<Grants>, q: ScriptQuestion) {
    if q.questions & TRIGGER_ANIMATION == 0 {
        return;
    }
    // the message names the owner, it gives no key
    let Some(used) = grants
        .use_name(&client, Grant::Animation, &q.owner_name)
        .await
    else {
        log::info!("{} asks to animate the bot: not allowed", q.object_name);
        return;
    };
    log::info!("{} may animate the bot ({used:?} grant)", q.object_name);
    if let Err(e) = client
        .answer_script(q.task, q.item, TRIGGER_ANIMATION)
        .await
    {
        log::warn!("script permissions: {e}");
    }
}

pub async fn run(client: Client, mut events: Receiver<Event>, grants: Arc<Grants>) {
    loop {
        match events.recv().await {
            Ok(Event::ScriptQuestion(q)) => {
                tokio::spawn(on_question(client.clone(), grants.clone(), q));
            }
            Ok(_) => {}
            Err(RecvError::Lagged(n)) => log::warn!("script permissions: {n} events missed"),
            Err(RecvError::Closed) => return,
        }
    }
}
