//! Teleport offers: the bot goes when a master offers (IM_LURE_USER), as SecondBot's
//! InteractionService accepts them from the names on its masters list (BotTeleportOffer). Others
//! are ignored, as SecondBot does with a teleport offer it rejects.
//!
//! Names are compared without case, and a username stands for "<name> Resident" (SecondBot's
//! "dot check"); SecondBot compares them exactly. Its other levels (Anyone, Friends) and its
//! AcceptTeleports switch come with the interaction rules (ROADMAP phase 5).

use ferro_client::events::dialog;
use ferro_client::{Client, Event};
use tokio::sync::broadcast::error::RecvError;

/// "First Last", lower case: "some" and "Some.Resident" are both "some resident".
fn canonical(name: &str) -> String {
    let parts: Vec<&str> = name.split([' ', '.']).filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [one] => format!("{one} resident"),
        parts => parts.join(" "),
    }
    .to_lowercase()
}

pub fn is_master(masters: &[String], name: &str) -> bool {
    let name = canonical(name);
    !name.is_empty() && masters.iter().any(|m| canonical(m) == name)
}

pub async fn run(client: Client, masters: Vec<String>) {
    let mut events = client.subscribe();
    loop {
        match events.recv().await {
            Ok(Event::InstantMessage(im)) if im.dialog == dialog::LURE_USER => {
                if !is_master(&masters, &im.from_name) {
                    log::info!("teleport offer from {} ignored: not a master", im.from_name);
                    continue;
                }
                log::info!("teleport offer from {}, accepted", im.from_name);
                let client = client.clone();
                tokio::spawn(async move {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masters_by_name_or_username_whatever_the_case() {
        let masters = vec!["Some Resident".to_owned(), "Other Avatar".to_owned()];
        assert!(is_master(&masters, "Some Resident"));
        assert!(is_master(&masters, "some"));
        assert!(is_master(&masters, "other.avatar"));
        assert!(is_master(&masters, "OTHER AVATAR"));
        assert!(!is_master(&masters, "Someone Else"));
        assert!(!is_master(&masters, ""));
        assert!(!is_master(&[], "Some Resident"));
    }
}
