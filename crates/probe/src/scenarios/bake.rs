//! Server-side baking and the asset service: the region bakes the bot's outfit, and the bakes
//! must then be there for anyone who draws the bot. Each bake is fetched as a viewer fetches it
//! (ViewerAsset), from the home region, from the neighbour after a teleport, and again after a
//! new login.

use super::{Online, Probe};
use ferro_client::appearance::{Appearance, bake_channel};
use ferro_client::{Client, Event};
use glam::Vec3;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Receiver;
use tokio::sync::broadcast::error::RecvError;
use uuid::Uuid;

/// How long a bake may take: the region fetches every wearable and texture, then composites.
pub(super) const BAKE_WAIT: Duration = Duration::from_secs(60);
/// A JPEG 2000 codestream starts with SOC then SIZ.
const J2K_MAGIC: [u8; 4] = [0xff, 0x4f, 0xff, 0x51];

impl Probe {
    /// The login's bake, a bake asked for (UpdateAvatarAppearance), every bake fetched from the
    /// home region and from the neighbour; then a new login, whose bakes must also be fetched.
    pub(super) async fn bake(&mut self, on: Online) -> Option<Online> {
        let me = on.client.state().agent_id;
        let client = on.client.clone();

        // the login bake: the region bakes when the agent becomes root
        let t = Instant::now();
        let login = match current(&client, me) {
            Some(a) => Some(a),
            None => next_appearance(&mut client.subscribe(), &client, me, BAKE_WAIT).await,
        };
        self.judge_bakes(&client, "bake-at-login", t, login.as_ref());

        // a bake asked for, as a viewer asks after changing the outfit
        let mut events = client.subscribe();
        let t = Instant::now();
        let asked = match client.rebake().await {
            Ok(_) => next_appearance(&mut events, &client, me, BAKE_WAIT).await,
            Err(e) => {
                self.emit(&client, "bake-asked", false, t, format!("refused: {e}"));
                None
            }
        };
        if asked.is_some() || login.is_none() {
            self.judge_bakes(&client, "bake-asked", t, asked.as_ref());
        }
        let Some(baked) = asked.or(login) else {
            return Some(on);
        };

        let home = self.opts.home.clone();
        self.fetch_bakes(&client, &format!("bake-fetch-in-{home}"), &baked)
            .await;
        self.fetch_from_service(&client, "bake-fetch-from-appearance-service", me, &baked)
            .await;
        if self.opts.neighbours {
            let near = self.opts.near.clone();
            let middle = Vec3::new(128.0, 128.0, 30.0);
            if self
                .teleport(&client, &format!("bake-tp-to-{near}"), &near, middle)
                .await
            {
                self.fetch_bakes(&client, &format!("bake-fetch-in-{near}"), &baked)
                    .await;
                self.teleport(&client, &format!("bake-tp-to-{home}"), &home, middle)
                    .await;
            }
        }

        // a new login: the bakes stored before must still be fetched, and the new ones too
        self.logout(on).await;
        let again = self.login("bake-relog").await?;
        let client = again.client.clone();
        let mut events = client.subscribe();
        let t = Instant::now();
        let first = match current(&client, me) {
            Some(a) => Some(a),
            None => next_appearance(&mut events, &client, me, BAKE_WAIT).await,
        };
        // the region may bake again a moment after the arrival; a viewer follows the latest
        // appearance, so the probe does too, once the appearances stop coming
        // the first bakes are what a viewer watching the arrival fetches: the appearance service must
        // serve them unless a later bake replaced them
        if let Some(first) = &first {
            self.fetch_from_service(&client, "bake-service-first-after-relog", me, first)
                .await;
        }
        let after = match first.clone() {
            Some(first) => Some(settled(&mut events, &client, me, first).await),
            None => None,
        };
        if let (Some(first), Some(after)) = (&first, &after) {
            let replaced = first.bakes() != after.bakes();
            self.emit(
                &client,
                "bake-first-was-replaced",
                true,
                t,
                if replaced {
                    "a later bake replaced the first ones".into()
                } else {
                    "the first bakes stayed".into()
                },
            );
        }
        self.judge_bakes(&client, "bake-after-relog", t, after.as_ref());
        self.fetch_bakes(&client, "bake-fetch-old-after-relog", &baked)
            .await;
        if let Some(after) = after {
            if after.bakes() != baked.bakes() {
                self.fetch_bakes(&client, "bake-fetch-new-after-relog", &after)
                    .await;
            }
            self.fetch_from_service(&client, "bake-service-after-relog", me, &after)
                .await;
        }
        Some(again)
    }

    /// An appearance with bakes came in time.
    fn judge_bakes(&mut self, client: &Client, what: &str, t: Instant, a: Option<&Appearance>) {
        let (ok, note) = match a {
            None => (false, format!("no appearance in {} s", BAKE_WAIT.as_secs())),
            Some(a) if a.bakes().is_empty() => (
                false,
                format!(
                    "an appearance with no bakes (COF version {})",
                    a.cof_version
                ),
            ),
            Some(a) => (
                true,
                format!("{} bakes, COF version {}", a.bakes().len(), a.cof_version),
            ),
        };
        self.emit(client, what, ok, t, note);
    }

    /// Every bake fetched as a viewer fetches it: a JPEG 2000 texture must come.
    pub(super) async fn fetch_bakes(&mut self, client: &Client, what: &str, a: &Appearance) {
        let t = Instant::now();
        let mut missing = Vec::new();
        for (face, id) in a.bakes() {
            match client.viewer_asset("texture_id", id).await {
                Ok(b) if b.starts_with(&J2K_MAGIC) => {}
                Ok(b) => missing.push(format!(
                    "face {face} {id}: {} bytes, not JPEG 2000",
                    b.len()
                )),
                Err(e) => missing.push(format!("face {face} {id}: {e}")),
            }
        }
        let n = a.bakes().len();
        let note = if missing.is_empty() {
            format!("{n} bakes")
        } else {
            format!("{} of {n} missing: {}", missing.len(), missing.join("; "))
        };
        self.emit(client, what, missing.is_empty() && n > 0, t, note);
    }
}

impl Probe {
    /// Every bake fetched as a viewer fetches another avatar's bakes when the region baked them
    /// (the appearance carries the outfit version): from the grid's appearance service, the
    /// login's `agent_appearance_service`, at `<url>texture/<agent>/<channel>/<bake>`.
    pub(super) async fn fetch_from_service(
        &mut self,
        client: &Client,
        what: &str,
        agent: Uuid,
        a: &Appearance,
    ) {
        let t = Instant::now();
        let Some(url) = client.state().agent_appearance_service.clone() else {
            self.emit(
                client,
                what,
                false,
                t,
                "the login gives no agent_appearance_service: a viewer fetches other avatars' \
                 server-side bakes there, so with none it textures nobody"
                    .into(),
            );
            return;
        };
        let mut missing = Vec::new();
        for (face, id) in a.bakes() {
            let Some(channel) = bake_channel(face) else {
                continue;
            };
            let sep = if url.ends_with('/') { "" } else { "/" };
            let get = format!("{url}{sep}texture/{agent}/{channel}/{id}");
            let got = match self
                .http
                .get(&get)
                .timeout(Duration::from_secs(30))
                .send()
                .await
            {
                Ok(r) if r.status().is_success() => r
                    .bytes()
                    .await
                    .map(|b| b.to_vec())
                    .map_err(|e| e.without_url().to_string()),
                Ok(r) => Err(format!("status {}", r.status().as_u16())),
                Err(e) => Err(e.without_url().to_string()),
            };
            match got {
                Ok(b) if b.starts_with(&J2K_MAGIC) => {}
                Ok(b) => missing.push(format!("{channel}: {} bytes, not JPEG 2000", b.len())),
                Err(e) => missing.push(format!("{channel}: {e}")),
            }
        }
        let n = a.bakes().len();
        let note = if missing.is_empty() {
            format!("{n} bakes")
        } else {
            format!("{} of {n} missing: {}", missing.len(), missing.join("; "))
        };
        self.emit(client, what, missing.is_empty() && n > 0, t, note);
    }
}

/// How long without a new appearance before the latest is taken as the one a viewer keeps.
const SETTLE: Duration = Duration::from_secs(10);

/// The latest appearance with bakes once the region has sent none for `SETTLE` (60 s at most).
pub(super) async fn settled(
    events: &mut Receiver<Event>,
    client: &Client,
    agent: Uuid,
    mut latest: Appearance,
) -> Appearance {
    let t = Instant::now();
    while t.elapsed() < BAKE_WAIT {
        match next_appearance(events, client, agent, SETTLE).await {
            Some(a) => latest = a,
            None => break,
        }
    }
    latest
}

/// The appearance the region already sent for `agent`, if it has bakes.
pub(super) fn current(client: &Client, agent: Uuid) -> Option<Appearance> {
    client
        .state()
        .appearances
        .get(&agent)
        .filter(|a| !a.bakes().is_empty())
        .cloned()
}

/// The next appearance with bakes the region sends for `agent` (one without, such as the
/// default sent on arrival, is passed over).
pub(super) async fn next_appearance(
    events: &mut Receiver<Event>,
    client: &Client,
    agent: Uuid,
    wait: Duration,
) -> Option<Appearance> {
    tokio::time::timeout(wait, async {
        loop {
            match events.recv().await {
                Ok(Event::Appearance { agent: a, .. }) if a == agent => {
                    if let Some(baked) = current(client, agent) {
                        return Some(baked);
                    }
                }
                Err(RecvError::Closed) => return None,
                _ => {}
            }
        }
    })
    .await
    .ok()
    .flatten()
}
