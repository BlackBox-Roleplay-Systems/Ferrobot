//! Sessions that end without the viewer saying so: a login whose viewer never connects, and a
//! viewer that goes quiet without logging out. Each is judged by the next login of the same
//! account: a region that still holds the old session takes the new login for a duplicate and
//! refuses it ("already logged in"), and the grid shows the user online meanwhile.

use super::{Online, Probe};
use ferro_client::{Client, End, login};
use std::time::{Duration, Instant};

/// How long a region may take to time out a viewer that sends nothing: OpenSim's AckTimeout
/// (60 s) and the check after it.
const VANISH_WAIT: Duration = Duration::from_secs(75);
/// How long a kick may take to reach the bot: the region waits half a second after KickUser.
const KICK_WAIT: Duration = Duration::from_secs(15);
/// How long the regions may take to drop the kicked agent, the neighbour's child agent included.
const KICK_GONE_WAIT: Duration = Duration::from_secs(15);

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

    /// An operator kicks the bot with the region console's `kick user`, run through remote
    /// admin: once plainly, once with `--force`. Each time the bot must be told why, in the
    /// operator's words; the home region and the neighbour (its child agent) must let the agent
    /// go; and the next login must work at once.
    pub(super) async fn kick(&mut self, on: Online) -> Option<Online> {
        let Some(admin) = self.admin.clone() else {
            return Some(on);
        };
        let mut on = on;
        for (what, flag) in [("kick", ""), ("kick-force", " --force")] {
            let Online {
                client,
                stop,
                mut session,
            } = on;
            let agent = client.state().agent_id.to_string();
            let mut regions = vec![self.opts.home.clone()];
            if self.opts.neighbours {
                regions.push(self.opts.near.clone());
            }
            // the agent must be in every region first (the neighbour's child agent comes a moment
            // after the arrival), or "gone" below would prove nothing
            for region in &regions {
                let t = Instant::now();
                let held = loop {
                    let held = admin.has_agent(region, &agent).await;
                    if matches!(held, Ok(true)) || t.elapsed() >= KICK_GONE_WAIT {
                        break held;
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                };
                let (ok, note) = match held {
                    Ok(true) => (true, String::new()),
                    Ok(false) => (false, format!("no agent in {region} to kick")),
                    Err(e) => (false, e),
                };
                self.emit(&client, &format!("{what}-setup-{region}"), ok, t, note);
            }
            let message = format!("Probe {what}, round {}", self.round);
            let command = format!("kick user {}{flag} {message}", self.user);
            let t = Instant::now();
            // timed to the bot's KickUser, not to the console command's end: the region finishes
            // closing the agent (saving the outfit among other things) after the user is told
            let (ran, (end, told)) = tokio::join!(admin.console(&command), async {
                let end = tokio::time::timeout(KICK_WAIT, &mut session).await;
                (end, t.elapsed())
            });
            // emit measures from its `since`: the time the bot was told
            let told = Instant::now().checked_sub(t.elapsed() - told).unwrap_or(t);
            if let Err(e) = ran {
                self.emit(&client, what, false, t, e);
                if !session.is_finished() {
                    self.logout(Online {
                        client,
                        stop,
                        session,
                    })
                    .await;
                }
                return None;
            }
            let (ok, note) = match end {
                Ok(Ok(End::Kicked(reason))) => {
                    let reason = reason.trim_matches(|c: char| c.is_whitespace() || c == '\0');
                    if reason == message {
                        (true, String::new())
                    } else {
                        (
                            false,
                            format!("kicked, but told {reason:?}, not {message:?}"),
                        )
                    }
                }
                Ok(Ok(end)) => (false, end.to_string()),
                Ok(Err(e)) => (false, format!("the session task: {e}")),
                Err(_) => (false, "still connected".into()),
            };
            self.emit(&client, what, ok, told, note);
            if !session.is_finished() {
                self.logout(Online {
                    client,
                    stop,
                    session,
                })
                .await;
                return None;
            }
            drop(stop);

            for region in &regions {
                let t = Instant::now();
                let held = loop {
                    let held = admin.has_agent(region, &agent).await;
                    if matches!(held, Ok(false)) || t.elapsed() >= KICK_GONE_WAIT {
                        break held;
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                };
                let (gone, note) = match held {
                    Ok(false) => (true, String::new()),
                    Ok(true) => (false, format!("{region} still holds the agent")),
                    Err(e) => (false, e),
                };
                self.emit(
                    &client,
                    &format!("{what}-gone-from-{region}"),
                    gone,
                    t,
                    note,
                );
            }

            on = self.login(&format!("{what}-then-login")).await?;
            // the regions settle the arrival (child agents) before the next kick
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        Some(on)
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
