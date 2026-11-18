//! Sessions that end without the viewer saying so: a login whose viewer never connects, and a
//! viewer that goes quiet without logging out. Each is judged by the next login of the same
//! account: a region that still holds the old session takes the new login for a duplicate and
//! refuses it ("already logged in"), and the grid shows the user online meanwhile.

use super::{Online, Probe};
use ferro_client::{Client, login};
use std::time::{Duration, Instant};

/// How long a region may take to time out a viewer that sends nothing: OpenSim's AckTimeout
/// (60 s) and the check after it.
const VANISH_WAIT: Duration = Duration::from_secs(75);

impl Probe {
    /// The bot logs out, then logs in without connecting (the login service answers, the viewer
    /// never comes: a cancelled login, a crash, UDP blocked), then waits `--login-wait` and logs
    /// in again. The region must have closed the first login by then.
    pub(super) async fn abandoned_login(&mut self, on: Online) -> Option<Online> {
        self.logout(on).await;
        let (client, _network) = Client::new(self.http.clone());
        let t = Instant::now();
        match login::login(&self.http, &self.login_request()).await {
            Ok(_) => self.emit(
                &client,
                "abandoned-login",
                true,
                t,
                "the viewer does not connect".into(),
            ),
            Err(e) => {
                self.emit(
                    &client,
                    "abandoned-login",
                    false,
                    t,
                    format!("refused: {e}"),
                );
                return self.login("abandoned-then-login").await;
            }
        }
        tokio::time::sleep(Duration::from_secs(self.opts.login_wait)).await;
        self.login("abandoned-then-login").await
    }

    /// The bot stops: no LogoutRequest, no packets, its socket closed. Once the region has timed
    /// it out the next login must work at once.
    pub(super) async fn vanish(&mut self, on: Online) -> Option<Online> {
        let Online {
            client,
            stop,
            session,
        } = on;
        let t = Instant::now();
        // ended before the stop signal can go: dropping it would make the session log out
        session.abort();
        let _ = session.await;
        drop(stop);
        self.emit(&client, "vanish", true, t, "no logout".into());
        tokio::time::sleep(VANISH_WAIT).await;
        self.login("vanish-then-login").await
    }
}
