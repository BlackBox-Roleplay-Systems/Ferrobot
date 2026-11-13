//! What the bot wears: attachments, and outfits through the Current Outfit Folder (its links say
//! what is worn; the region bakes the avatar from it), as LibreMetaverse's AppearanceManager and
//! CurrentOutfitFolder do for SecondBot, and LLAppearanceMgr for the viewer.

use crate::inventory::InvItem;
use crate::network::{InventoryRequest, ObjectRequest};
use crate::objects::Worn;
use crate::{Client, ClientError, Request};
use std::collections::HashSet;
use std::time::Duration;
use uuid::Uuid;

/// LLFolderType::FT_CURRENT_OUTFIT
const CURRENT_OUTFIT: i32 = 46;
/// LLAssetType: clothing, an object, a body part, a link.
const CLOTHING: i32 = 5;
const OBJECT: i32 = 6;
const BODYPART: i32 = 13;
const LINK: i32 = 24;
/// Attachments asked for one at a time (Aurora's attach_queue: bursts fail on a busy region).
const ATTACH_SPACING: Duration = Duration::from_millis(250);
/// The region's links settle before the bake is asked for.
const BEFORE_BAKE: Duration = Duration::from_secs(2);

/// A wearable's type, in the low byte of its item flags (LLWearableType).
fn wearable_type(i: &InvItem) -> u32 {
    i.flags & 0xff
}

/// What an outfit folder or the Current Outfit Folder holds, links followed.
#[derive(Debug, Clone)]
struct Entry {
    /// The link itself, when it is one.
    link: Option<Uuid>,
    item: InvItem,
}

impl Client {
    async fn object_ask<T>(
        &self,
        make: impl FnOnce(tokio::sync::oneshot::Sender<T>) -> ObjectRequest,
    ) -> Result<T, ClientError> {
        self.ask(|reply| Request::Object(make(reply))).await
    }

    /// What the bot wears, as the region's object updates tell it.
    pub async fn worn(&self) -> Result<Vec<Worn>, ClientError> {
        self.object_ask(|reply| ObjectRequest::Worn { reply }).await
    }

    /// The Current Outfit Folder.
    pub async fn current_outfit_folder(&self) -> Result<Uuid, ClientError> {
        let root = self.inventory_root();
        self.folder_contents(root)
            .await?
            .folders
            .into_iter()
            .find(|f| f.type_default == CURRENT_OUTFIT)
            .map(|f| f.id)
            .ok_or_else(|| ClientError::Refused("no Current Outfit Folder".into()))
    }

    /// A folder's wearables and objects, links followed to what they point to.
    async fn wearables_in(&self, folder: Uuid) -> Result<Vec<Entry>, ClientError> {
        let mut out = Vec::new();
        for i in self.folder_contents(folder).await?.items {
            if i.asset_type == LINK {
                if let Ok(Some(target)) = self.inventory_item(i.asset_id).await {
                    out.push(Entry {
                        link: Some(i.id),
                        item: target,
                    });
                }
            } else {
                out.push(Entry {
                    link: None,
                    item: i,
                });
            }
        }
        out.retain(|e| matches!(e.item.asset_type, CLOTHING | OBJECT | BODYPART));
        Ok(out)
    }

    async fn link(&self, folder: Uuid, target: &InvItem) -> Result<(), ClientError> {
        let target = Box::new(target.clone());
        self.ask(|reply| {
            Request::Inventory(InventoryRequest::Link {
                folder,
                target,
                reply,
            })
        })
        .await
    }

    /// Removes items from the inventory for good (RemoveInventoryItem), not to the Trash.
    pub async fn remove_items(&self, items: Vec<Uuid>) -> Result<(), ClientError> {
        if items.is_empty() {
            return Ok(());
        }
        self.ask(|reply| Request::Inventory(InventoryRequest::Remove { items, reply }))
            .await
    }

    /// An item, a link followed.
    async fn real_item(&self, id: Uuid) -> Result<InvItem, ClientError> {
        let i = self
            .inventory_item(id)
            .await?
            .ok_or_else(|| ClientError::Refused("Unable to find item".into()))?;
        if i.asset_type != LINK {
            return Ok(i);
        }
        self.inventory_item(i.asset_id)
            .await?
            .ok_or_else(|| ClientError::Refused("Unable to find item".into()))
    }

    /// Wear an object, added to what is worn, and keep it worn (a link in the Current Outfit
    /// Folder).
    pub async fn attach(&self, item: Uuid) -> Result<(), ClientError> {
        let item = self.real_item(item).await?;
        if item.asset_type != OBJECT {
            return Err(ClientError::Refused("not an object".into()));
        }
        let cof = self.current_outfit_folder().await?;
        let linked = self
            .wearables_in(cof)
            .await?
            .iter()
            .any(|e| e.item.id == item.id);
        if !linked {
            self.link(cof, &item).await?;
        }
        let item = Box::new(item);
        self.object_ask(|reply| ObjectRequest::Attach { item, reply })
            .await
    }

    /// Take off what was worn from an item, and its links in the Current Outfit Folder.
    pub async fn detach(&self, item: Uuid) -> Result<(), ClientError> {
        self.object_ask(|reply| ObjectRequest::Detach { item, reply })
            .await?;
        let cof = self.current_outfit_folder().await?;
        let links: Vec<Uuid> = self
            .wearables_in(cof)
            .await?
            .into_iter()
            .filter(|e| e.item.id == item)
            .filter_map(|e| e.link)
            .collect();
        self.remove_items(links).await
    }

    /// Wear an outfit folder: its clothing replaces all clothing, its body parts those of their
    /// types (a type it lacks keeps the one worn: an avatar always has a shape, skin, hair and
    /// eyes), its objects replace the objects worn. Then the region bakes the avatar again.
    pub async fn replace_outfit(&self, folder: Uuid) -> Result<(), ClientError> {
        let new = self.wearables_in(folder).await?;
        if new.is_empty() {
            return Err(ClientError::Refused(
                "nothing to wear in that folder".into(),
            ));
        }
        let cof = self.current_outfit_folder().await?;
        let current = self.wearables_in(cof).await?;
        let new_ids: HashSet<Uuid> = new.iter().map(|e| e.item.id).collect();
        let new_bodyparts: HashSet<u32> = new
            .iter()
            .filter(|e| e.item.asset_type == BODYPART)
            .map(|e| wearable_type(&e.item))
            .collect();
        // the links that go: clothing, replaced body parts, objects not in the new outfit
        let gone: Vec<&Entry> = current
            .iter()
            .filter(|e| match e.item.asset_type {
                CLOTHING => !new_ids.contains(&e.item.id),
                BODYPART => {
                    new_bodyparts.contains(&wearable_type(&e.item)) && !new_ids.contains(&e.item.id)
                }
                _ => !new_ids.contains(&e.item.id),
            })
            .collect();
        self.remove_items(gone.iter().filter_map(|e| e.link).collect())
            .await?;
        let linked: HashSet<Uuid> = current.iter().map(|e| e.item.id).collect();
        for e in new.iter().filter(|e| !linked.contains(&e.item.id)) {
            self.link(cof, &e.item).await?;
        }
        // the objects: off what is not in it, on what is not worn yet
        let worn = self.worn().await?;
        for w in worn.iter().filter(|w| !new_ids.contains(&w.item)) {
            let item = w.item;
            self.object_ask(|reply| ObjectRequest::Detach { item, reply })
                .await?;
        }
        let wearing: HashSet<Uuid> = worn.iter().map(|w| w.item).collect();
        for e in new
            .iter()
            .filter(|e| e.item.asset_type == OBJECT && !wearing.contains(&e.item.id))
        {
            let item = Box::new(e.item.clone());
            self.object_ask(|reply| ObjectRequest::Attach { item, reply })
                .await?;
            tokio::time::sleep(ATTACH_SPACING).await;
        }
        tokio::time::sleep(BEFORE_BAKE).await;
        self.rebake().await.map(|_| ())
    }
}
