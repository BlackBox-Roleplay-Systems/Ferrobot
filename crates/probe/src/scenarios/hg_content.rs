//! Content made on another grid (Hypergrid): the visitor takes a box it made there into its
//! inventory and writes a notecard there, then goes home and uses both. The items land in the
//! home inventory; this checks whether the assets behind them came home too.

use super::Probe;
use crate::options::hg_region_name;
use ferro_client::Client;
use ferro_client::inventory::InvItem;
use glam::Vec3;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use uuid::Uuid;

const SETTLE: Duration = Duration::from_secs(5);

/// Every item in the bot's inventory, by id (the folders fetched a few at a time).
async fn all_items(client: &Client) -> HashMap<Uuid, InvItem> {
    let mut items = HashMap::new();
    let mut todo = vec![client.inventory_root()];
    while !todo.is_empty() {
        let batch: Vec<Uuid> = todo.drain(..todo.len().min(8)).collect();
        let Ok(folders) = client.fetch_folders(&batch).await else {
            break;
        };
        for f in folders {
            todo.extend(f.folders.iter().map(|s| s.id));
            for i in f.items {
                items.insert(i.id, i);
            }
        }
    }
    items
}

impl Probe {
    pub(super) async fn hg_content(&mut self, client: &Client) {
        let o = self.opts.clone();
        let Some(address) = o.hg_region.clone() else {
            return;
        };
        let Some(there) = hg_region_name(&address).map(str::to_owned) else {
            return;
        };
        let middle = Vec3::new(128.0, 128.0, 30.0);
        let me = client.state().agent_id;
        let before = all_items(client).await;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let note_name = format!("hg-note-{stamp}");
        let note_text = format!("written on {there} at {stamp}");

        if !self
            .teleport_via(
                client,
                &format!("hgc-tp-to-{there}"),
                &address,
                &there,
                middle,
            )
            .await
        {
            return;
        }
        tokio::time::sleep(SETTLE).await;

        // a box made there, taken into the inventory
        let t = Instant::now();
        let roots_before: std::collections::HashSet<Uuid> = client
            .root_objects()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| r.full_id)
            .collect();
        let at = client.state().position + Vec3::new(3.0, 0.0, 0.0);
        let made = client.add_box(at, Vec3::new(1.0, 1.0, 0.5)).await;
        let mut the_box = None;
        while made.is_ok() && the_box.is_none() && t.elapsed() < Duration::from_secs(10) {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let near: Vec<_> = client
                .root_objects()
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|r| !roots_before.contains(&r.full_id) && r.position.distance(at) < 3.0)
                .collect();
            if near.is_empty() {
                continue;
            }
            let _ = client
                .learn_objects(
                    near.iter().map(|r| r.full_id).collect(),
                    Duration::from_secs(3),
                )
                .await;
            the_box = client.root_objects().await.ok().and_then(|roots| {
                roots
                    .into_iter()
                    .find(|r| near.iter().any(|n| n.full_id == r.full_id) && r.owner == me)
            });
        }
        let Some(the_box) = the_box else {
            self.emit(client, "hgc-box-made-abroad", false, t, format!("{made:?}"));
            return;
        };
        self.emit(
            client,
            "hgc-box-made-abroad",
            true,
            t,
            the_box.full_id.to_string(),
        );
        let t = Instant::now();
        let taken = client.derez(the_box.local_id, Uuid::nil()).await;
        let mut gone = false;
        while taken.is_ok() && t.elapsed() < Duration::from_secs(10) {
            tokio::time::sleep(Duration::from_millis(200)).await;
            gone = !client
                .root_objects()
                .await
                .unwrap_or_default()
                .iter()
                .any(|r| r.full_id == the_box.full_id);
            if gone {
                break;
            }
        }
        self.emit(
            client,
            "hgc-box-taken-abroad",
            gone,
            t,
            format!("{taken:?}"),
        );

        // a notecard written there
        let t = Instant::now();
        let note = client.create_notecard(&note_name, "", &note_text).await;
        self.emit(
            client,
            "hgc-notecard-written-abroad",
            note.is_ok(),
            t,
            format!("{note:?}"),
        );
        tokio::time::sleep(SETTLE).await;

        let home = o.home.clone();
        let back = format!("{}:{home}", o.home_gate());
        if !self
            .teleport_via(
                client,
                &format!("hgc-tp-home-to-{home}"),
                &back,
                &home,
                middle,
            )
            .await
        {
            return;
        }
        tokio::time::sleep(SETTLE).await;

        // what came home: the new items, and whether their assets work here
        let t = Instant::now();
        let after = all_items(client).await;
        let new: Vec<&InvItem> = after
            .values()
            .filter(|i| !before.contains_key(&i.id))
            .collect();
        let object = new.iter().find(|i| i.asset_type == 6).copied().cloned();
        let card = new
            .iter()
            .find(|i| i.asset_type == 7 && i.name == note_name)
            .copied()
            .cloned();
        self.emit(
            client,
            "hgc-items-at-home",
            object.is_some() && card.is_some(),
            t,
            format!(
                "object item {:?} asset {:?}; notecard item {:?} asset {:?}",
                object.as_ref().map(|i| i.id),
                object.as_ref().map(|i| i.asset_id),
                card.as_ref().map(|i| i.id),
                card.as_ref().map(|i| i.asset_id)
            ),
        );

        if let Some(item) = object {
            let t = Instant::now();
            let roots_before: std::collections::HashSet<Uuid> = client
                .root_objects()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|r| r.full_id)
                .collect();
            let at = client.state().position + Vec3::new(3.0, 0.0, 0.0);
            let asked = client.rez(item, at).await;
            let mut rezzed = None;
            while asked.is_ok() && rezzed.is_none() && t.elapsed() < Duration::from_secs(15) {
                tokio::time::sleep(Duration::from_millis(250)).await;
                rezzed = client
                    .root_objects()
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .find(|r| !roots_before.contains(&r.full_id) && r.position.distance(at) < 4.0);
            }
            self.emit(
                client,
                "hgc-rez-at-home",
                rezzed.is_some(),
                t,
                if rezzed.is_some() {
                    String::new()
                } else {
                    format!("nothing rezzed ({asked:?})")
                },
            );
            if let Some(r) = rezzed {
                let _ = client.delete_object(r.local_id).await;
            }
        }
        if let Some(item) = card {
            let t = Instant::now();
            let read = client.read_notecard(&item, "").await;
            let ok = read.as_deref().is_ok_and(|s| s.contains(&note_text));
            self.emit(
                client,
                "hgc-notecard-read-at-home",
                ok,
                t,
                if ok {
                    String::new()
                } else {
                    format!("{read:?}")
                },
            );
        }
        self.after(client, "after-hgc").await;
    }
}
