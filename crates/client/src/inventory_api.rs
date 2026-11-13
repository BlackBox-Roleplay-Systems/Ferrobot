//! The client's inventory: folders and their contents (fetched when asked), items, giving,
//! creating, moving, renaming and throwing away, as SecondBot's Inventory commands need them.

use crate::inventory::{FolderContents, InvFolder, InvItem};
use crate::network::InventoryRequest;
use crate::{Client, ClientError, Request};
use serde::Serialize;
use std::time::Duration;
use tokio::sync::oneshot;
use uuid::Uuid;

const FETCH_TIMEOUT: Duration = Duration::from_secs(45);
/// Folders per FetchInventoryDescendents2 request, walking the tree.
const FOLDERS_PER_FETCH: usize = 10;
/// The Trash's preferred type (LLFolderType::FT_TRASH).
const TRASH: i32 = 14;
/// A link's asset type (LLAssetType::AT_LINK).
const LINK: i32 = 24;
const CATEGORY: u8 = 8;

/// A folder and those under it (SecondBot's InventoryMapFolder).
#[derive(Debug, Clone, Serialize)]
pub struct FolderNode {
    pub id: String,
    pub name: String,
    pub subfolders: Vec<FolderNode>,
}

impl Client {
    async fn inventory_ask<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<T>) -> InventoryRequest,
    ) -> Result<T, ClientError> {
        self.ask_within(FETCH_TIMEOUT, |reply| Request::Inventory(make(reply)))
            .await
    }

    /// The inventory's root folder (the login's inventory-root).
    pub fn inventory_root(&self) -> Uuid {
        self.shared.state().inventory_root
    }

    /// Folders' contents (FetchInventoryDescendents2).
    pub async fn fetch_folders(
        &self,
        folders: &[Uuid],
    ) -> Result<Vec<FolderContents>, ClientError> {
        let folders = folders.to_vec();
        let found = self
            .inventory_ask(|reply| InventoryRequest::Fetch { folders, reply })
            .await?
            .map_err(ClientError::Refused)?;
        let mut names = self
            .shared
            .folder_names
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for f in found.iter().flat_map(|c| &c.folders) {
            names.insert(f.id, f.name.clone());
        }
        Ok(found)
    }

    /// A folder's name, when a fetch has shown it ("My Inventory" for the root).
    pub fn folder_name(&self, folder: Uuid) -> Option<String> {
        if folder == self.inventory_root() {
            return Some("My Inventory".into());
        }
        self.shared
            .folder_names
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&folder)
            .cloned()
    }

    /// One folder's contents.
    pub async fn folder_contents(&self, folder: Uuid) -> Result<FolderContents, ClientError> {
        self.fetch_folders(&[folder])
            .await?
            .into_iter()
            .find(|f| f.folder_id == folder)
            .ok_or_else(|| ClientError::Refused("no such folder".into()))
    }

    /// The folders under `root`, all of them (or only its own, `deep` false), fetched a few
    /// at a time.
    pub async fn folder_tree(
        &self,
        root: Uuid,
        name: &str,
        deep: bool,
    ) -> Result<FolderNode, ClientError> {
        let mut names = std::collections::HashMap::new();
        let mut children: std::collections::HashMap<Uuid, Vec<Uuid>> = Default::default();
        names.insert(root, name.to_owned());
        let mut todo = vec![root];
        while !todo.is_empty() {
            let batch: Vec<Uuid> = todo.drain(..todo.len().min(FOLDERS_PER_FETCH)).collect();
            for f in self.fetch_folders(&batch).await? {
                for sub in &f.folders {
                    names.insert(sub.id, sub.name.clone());
                    children.entry(f.folder_id).or_default().push(sub.id);
                    if deep {
                        todo.push(sub.id);
                    }
                }
            }
        }
        fn build(
            id: Uuid,
            names: &std::collections::HashMap<Uuid, String>,
            children: &std::collections::HashMap<Uuid, Vec<Uuid>>,
        ) -> FolderNode {
            FolderNode {
                id: id.to_string(),
                name: names.get(&id).cloned().unwrap_or_default(),
                subfolders: children
                    .get(&id)
                    .into_iter()
                    .flatten()
                    .map(|c| build(*c, names, children))
                    .collect(),
            }
        }
        Ok(build(root, &names, &children))
    }

    /// An item of the bot's (FetchInventory2), `None` when there is none.
    pub async fn inventory_item(&self, item: Uuid) -> Result<Option<InvItem>, ClientError> {
        let items = vec![item];
        Ok(self
            .inventory_ask(|reply| InventoryRequest::FetchItems { items, reply })
            .await?
            .map_err(ClientError::Refused)?
            .into_iter()
            .find(|i| i.id == item))
    }

    /// The asset behind an item, a link followed (SecondBot's GetRealUUID).
    pub async fn item_asset(&self, item: Uuid) -> Result<Option<Uuid>, ClientError> {
        let Some(i) = self.inventory_item(item).await? else {
            return Ok(None);
        };
        if i.asset_type != LINK {
            return Ok(Some(i.asset_id));
        }
        Ok(self.inventory_item(i.asset_id).await?.map(|t| t.asset_id))
    }

    async fn give(
        &self,
        to: Uuid,
        name: String,
        entries: Vec<(u8, Uuid)>,
    ) -> Result<(), ClientError> {
        self.inventory_ask(|reply| InventoryRequest::Give {
            to,
            name,
            entries,
            reply,
        })
        .await
    }

    /// Give an item to an avatar (an inventory offer they accept or not).
    pub async fn give_item(&self, to: Uuid, item: Uuid) -> Result<(), ClientError> {
        let Some(i) = self.inventory_item(item).await? else {
            return Err(ClientError::Refused("Unable to find item".into()));
        };
        let t = u8::try_from(i.asset_type).unwrap_or(0);
        self.give(to, i.name, vec![(t, i.id)]).await
    }

    /// A notecard's text (its asset downloaded as the viewer does, the region checking the
    /// bot may read it); each embedded item becomes `embedded`.
    pub async fn read_notecard(
        &self,
        item: &InvItem,
        embedded: &str,
    ) -> Result<String, ClientError> {
        let item = item.clone();
        let data = self
            .inventory_ask(|reply| InventoryRequest::ReadAsset { item, reply })
            .await?
            .map_err(ClientError::Refused)?;
        crate::transfer::notecard_text(&data, embedded)
            .ok_or_else(|| ClientError::Refused("not a notecard".into()))
    }

    /// Give an item by its key, name and asset type, without looking it up (SecondBot's
    /// SendItemDirect).
    pub async fn give_item_as(
        &self,
        to: Uuid,
        item: Uuid,
        name: &str,
        asset_type: u8,
    ) -> Result<(), ClientError> {
        self.give(to, name.to_owned(), vec![(asset_type, item)])
            .await
    }

    /// The folder at the end of the folders named in `parts`, from the root (SecondBot's
    /// FindObjectByPath walks the same way: exact names, one folder fetched per part).
    async fn walk(&self, parts: &[&str]) -> Result<Option<Uuid>, ClientError> {
        let mut current = self.inventory_root();
        for part in parts {
            let contents = self.folder_contents(current).await?;
            match contents.folders.iter().find(|f| f.name == *part) {
                Some(f) => current = f.id,
                None => return Ok(None),
            }
        }
        Ok(Some(current))
    }

    /// An item by its path from the root, "Objects/Demo Item"; empty parts are skipped.
    pub async fn item_by_path(&self, path: &str) -> Result<Option<InvItem>, ClientError> {
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let Some((name, folders)) = parts.split_last() else {
            return Ok(None);
        };
        let Some(folder) = self.walk(folders).await? else {
            return Ok(None);
        };
        let contents = self.folder_contents(folder).await?;
        Ok(contents.items.into_iter().find(|i| i.name == *name))
    }

    /// A folder by its path from the root, "Objects/Gifts".
    pub async fn folder_by_path(&self, path: &str) -> Result<Option<InvFolder>, ClientError> {
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let Some((name, parents)) = parts.split_last() else {
            return Ok(None);
        };
        let Some(parent) = self.walk(parents).await? else {
            return Ok(None);
        };
        let contents = self.folder_contents(parent).await?;
        Ok(contents.folders.into_iter().find(|f| f.name == *name))
    }

    /// Give a folder and the items in it (LLGiveInventory::giveInventoryCategory: the folder, then
    /// each item, in the bucket).
    pub async fn give_folder(&self, to: Uuid, folder: Uuid, name: &str) -> Result<(), ClientError> {
        let contents = self.folder_contents(folder).await?;
        let mut entries = vec![(CATEGORY, folder)];
        entries.extend(
            contents
                .items
                .iter()
                .map(|i| (u8::try_from(i.asset_type).unwrap_or(0), i.id)),
        );
        self.give(to, name.to_owned(), entries).await
    }

    /// Create a folder; answers its key.
    pub async fn create_folder(&self, parent: Uuid, name: &str) -> Result<Uuid, ClientError> {
        let id = Uuid::new_v4();
        let name = name.to_owned();
        self.inventory_ask(|reply| InventoryRequest::CreateFolder {
            id,
            parent,
            name,
            reply,
        })
        .await?;
        Ok(id)
    }

    /// Move an item to a folder, and rename it when `name` is given.
    pub async fn move_item(
        &self,
        item: Uuid,
        folder: Uuid,
        name: Option<&str>,
    ) -> Result<(), ClientError> {
        let name = name.map(str::to_owned);
        self.inventory_ask(|reply| InventoryRequest::MoveItem {
            item,
            folder,
            name,
            reply,
        })
        .await
    }

    /// Rename an item where it is.
    pub async fn rename_item(&self, item: Uuid, name: &str) -> Result<(), ClientError> {
        let Some(i) = self.inventory_item(item).await? else {
            return Err(ClientError::Refused("Unable to find inventory item".into()));
        };
        self.move_item(item, i.parent, Some(name)).await
    }

    pub async fn move_folder(&self, folder: Uuid, parent: Uuid) -> Result<(), ClientError> {
        self.inventory_ask(|reply| InventoryRequest::MoveFolder {
            folder,
            parent,
            reply,
        })
        .await
    }

    /// The Trash folder, in the root.
    pub async fn trash_folder(&self) -> Result<Uuid, ClientError> {
        self.folder_contents(self.inventory_root())
            .await?
            .folders
            .into_iter()
            .find(|f| f.type_default == TRASH)
            .map(|f| f.id)
            .ok_or_else(|| ClientError::Refused("no Trash folder".into()))
    }

    /// Throw an item away: to the Trash, as the viewer's delete does.
    pub async fn trash_item(&self, item: Uuid) -> Result<(), ClientError> {
        let trash = self.trash_folder().await?;
        self.move_item(item, trash, None).await
    }

    /// Throw a folder away: to the Trash.
    pub async fn trash_folder_of(&self, folder: Uuid) -> Result<(), ClientError> {
        let trash = self.trash_folder().await?;
        self.move_folder(folder, trash).await
    }

    /// Create a notecard with this text, in the Notecards folder (else the root); answers its key.
    pub async fn create_notecard(&self, name: &str, text: &str) -> Result<Uuid, ClientError> {
        let root = self.inventory_root();
        let folder = self
            .folder_contents(root)
            .await
            .ok()
            .and_then(|c| c.folders.into_iter().find(|f| f.type_default == 7))
            .map_or(root, |f| f.id);
        let (name, description) = (name.to_owned(), String::new());
        let item = self
            .inventory_ask(|reply| InventoryRequest::CreateItem {
                folder,
                name,
                description,
                asset_type: 7,
                inv_type: 7,
                reply,
            })
            .await?;
        let data = crate::inventory::notecard_asset(text);
        self.inventory_ask(|reply| InventoryRequest::UploadNotecard { item, data, reply })
            .await?
            .map_err(ClientError::Refused)?;
        Ok(item)
    }

    /// Create a notecard and give it (LibreMetaverse's SendNotecard, SecondBot's
    /// NotecardDirectSend).
    pub async fn send_notecard(&self, to: Uuid, name: &str, text: &str) -> Result<(), ClientError> {
        let item = self.create_notecard(name, text).await?;
        self.give(to, name.to_owned(), vec![(7, item)]).await
    }
}
