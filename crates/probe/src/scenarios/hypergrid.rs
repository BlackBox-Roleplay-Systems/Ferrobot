//! A trip to another grid (Hypergrid): the bot leaves its home grid for a region of the other,
//! moves about there as a visitor, and comes back. The other grid trusts nothing of the home
//! grid: its gatekeeper lets the visitor in, and the regions talk to each other directly.

use super::Probe;
use crate::options::hg_region_name;
use ferro_client::Client;
use glam::Vec3;
use std::time::{Duration, Instant};

/// How long the regions may take to settle an arrival from another grid (the visitor's
/// appearance, inventory and child agents come from its home grid).
const SETTLE: Duration = Duration::from_secs(5);

impl Probe {
    /// To the region of the other grid; a teleport inside it; a crossing to its neighbour and
    /// back (child agents of a visitor); home through the other grid's map; then the home grid's
    /// own teleports, to see the trip left nothing behind.
    pub(super) async fn hypergrid(&mut self, client: &Client) {
        let o = self.opts.clone();
        let Some(address) = o.hg_region.clone() else {
            return;
        };
        let Some(there) = hg_region_name(&address).map(str::to_owned) else {
            return;
        };
        let middle = Vec3::new(128.0, 128.0, 30.0);

        if !self
            .teleport_via(
                client,
                &format!("hg-tp-to-{there}"),
                &address,
                &there,
                middle,
            )
            .await
        {
            self.after(client, "after-hg-refused").await;
            return;
        }
        tokio::time::sleep(SETTLE).await;

        self.teleport(
            client,
            &format!("hg-local-tp-{there}"),
            &there,
            Vec3::new(60.0, 60.0, 30.0),
        )
        .await;
        if let Some(near) = o.hg_near.clone()
            && self
                .teleport(
                    client,
                    "hg-cross-setup",
                    &there,
                    Vec3::new(230.0, 128.0, 40.0),
                )
                .await
        {
            // the visitor's position updates reach the neighbour's child agent on the way
            tokio::time::sleep(Duration::from_secs(3)).await;
            self.cross(client, "hg-cross-east", &near, true).await;
            self.cross(client, "hg-cross-west", &there, false).await;
        }

        let home = o.home.clone();
        let back = format!("{}:{home}", o.home_gate());
        let t = Instant::now();
        if !self
            .teleport_via(
                client,
                &format!("hg-tp-home-to-{home}"),
                &back,
                &home,
                middle,
            )
            .await
        {
            let st = client.state();
            self.emit(
                client,
                "hg-stranded",
                false,
                t,
                format!("still on the other grid; online {}", st.online),
            );
            return;
        }
        tokio::time::sleep(SETTLE).await;
        self.after(client, "after-hg").await;
    }
}
