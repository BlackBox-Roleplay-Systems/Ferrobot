//! Region restarts: when the region announces one, the bot leaves for home if home is in another
//! region; if home is here too, it stays, and logs in again once the region is back (the relog
//! loop). Second Life gives a few minutes' notice; the teleport goes at once.

use ferro_client::{Client, Event};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;

pub async fn run(client: Client, mut events: Receiver<Event>) {
    loop {
        match events.recv().await {
            Ok(Event::RegionRestart { seconds, .. }) => {
                let st = client.state();
                let here = st.region.as_ref().map(|r| r.handle);
                match st.home {
                    Some(home) if Some(home.region_handle) != here => {
                        log::warn!("the region restarts in {seconds} s: going home");
                        let client = client.clone();
                        tokio::spawn(async move {
                            match client.teleport_home().await {
                                Ok(region) => log::info!("home, in {region}, before the restart"),
                                Err(e) => log::warn!(
                                    "could not go home before the restart: {e}; the bot will log in again after it"
                                ),
                            }
                        });
                    }
                    _ => log::warn!(
                        "the region restarts in {seconds} s; home is not elsewhere, the bot will log in again after it"
                    ),
                }
            }
            Ok(_) => {}
            Err(RecvError::Lagged(n)) => log::warn!("restart: {n} events missed"),
            Err(RecvError::Closed) => return,
        }
    }
}
