//! SecondBot's Inventory commands that need no objects in world: the folders, a folder's
//! contents, sending items and folders, creating, renaming, moving and deleting, purging old
//! notecards, and the inventory update events.
//!
//! Deliberate differences: deleting moves to the Trash, as the viewer does (SecondBot removes for
//! good; InventoryPurgeNotecards does too, as SecondBot's); InventoryFoldersLimited lists the
//! folder's own subfolders (SecondBot's gives the folder alone). Ferrobot keeps no inventory
//! cache and does not preload: InventoryContentsCatched fetches the whole tree when asked,
//! InventoryPreloadDone is always "Ready" (as SecondBot's with its preload off), and
//! InventoryPreloadCount counts what the fetches have shown so far. SecondBot's
//! InventoryPurgeNotecards fails when it finds the folder (an inverted check): Ferrobot's fails
//! when it does not.
//!
//! UploadMediaWave cannot do what SecondBot's does: SecondBot reads a WAV file from a path on its
//! own disk and encodes it to Ogg Vorbis, and the container has neither the files nor an audio
//! library. Ferrobot's downloads a sound already encoded (Ogg Vorbis, mono, 44100 Hz, 30 s at
//! most, as the viewer makes them) from an http(s) URL, checks its headers, and uploads it. It
//! costs what the grid asks for a sound, so it is a Funds command unless the grid says it is free.

use super::reply::{Answer, basic, failure, json};
use super::{Ctx, avatar, key, owned};
use serde::Serialize;

/// SecondBot's InventoryMapItem.
#[derive(Serialize)]
struct MapItem {
    id: String,
    name: String,
    typename: String,
}

/// The LibreMetaverse class an inventory type makes (InventoryItem.Create), which is what
/// SecondBot writes as `typename`.
fn typename(inv_type: i32) -> &'static str {
    match inv_type {
        0 => "InventoryTexture",
        1 => "InventorySound",
        2 => "InventoryCallingCard",
        3 => "InventoryLandmark",
        6 => "InventoryObject",
        7 => "InventoryNotecard",
        8 => "InventoryCategory",
        10 => "InventoryLSL",
        15 => "InventorySnapshot",
        17 => "InventoryAttachment",
        18 => "InventoryWearable",
        19 => "InventoryAnimation",
        20 => "InventoryGesture",
        25 => "InventorySettings",
        26 => "InventoryMaterial",
        _ => "InventoryItem",
    }
}

pub async fn folders(ctx: &Ctx) -> Answer {
    let root = ctx.client.inventory_root();
    match ctx.client.folder_tree(root, "My Inventory", true).await {
        Ok(tree) => basic("InventoryFolders", &json(&tree), &[]),
        Err(_) => failure("InventoryFolders", "Error", &[]),
    }
}

pub async fn folders_limited(ctx: &Ctx, folder: &str) -> Answer {
    const C: &str = "InventoryFoldersLimited";
    let Some(id) = key(folder) else {
        return failure(C, "Error", &[]);
    };
    let name = ctx.client.folder_name(id).unwrap_or_default();
    match ctx.client.folder_tree(id, &name, false).await {
        Ok(tree) => basic(C, &json(&tree), &[]),
        Err(_) => failure(C, "Error", &[]),
    }
}

/// InventoryContents: the folder first ("Folder"), then what is in it.
pub async fn contents(ctx: &Ctx, folder: &str) -> Answer {
    const C: &str = "InventoryContents";
    let args = owned(&[folder]);
    let Some(id) = key(folder) else {
        return failure(C, "Invaild folder UUID", &args);
    };
    basic(C, &contents_json(ctx, id).await, &args)
}

/// SecondBot's MapFolderInventoryJson: the folder, then what is in it.
async fn contents_json(ctx: &Ctx, id: uuid::Uuid) -> String {
    let mut entries = vec![MapItem {
        id: id.to_string(),
        name: ctx.client.folder_name(id).unwrap_or_default(),
        typename: "Folder".into(),
    }];
    if let Ok(c) = ctx.client.folder_contents(id).await {
        entries.extend(c.folders.iter().map(|f| MapItem {
            id: f.id.to_string(),
            name: f.name.clone(),
            typename: "InventoryFolder".into(),
        }));
        entries.extend(c.items.iter().map(|i| MapItem {
            id: i.id.to_string(),
            name: i.name.clone(),
            typename: typename(i.inv_type).into(),
        }));
    }
    json(&entries)
}

/// SecondBot's InventoryAsyncReply, its fields in their order.
#[derive(Serialize)]
pub(super) struct AsyncReply {
    #[serde(rename = "requestUUID")]
    pub request: String,
    pub status: bool,
    pub message: String,
    pub data: String,
}

/// The answer of an Async… command, to its reply target.
pub(super) async fn async_reply(ctx: &Ctx, target: &str, command: &str, r: AsyncReply) {
    crate::smart_reply::send(ctx, target, &json(&r), command).await;
}

/// AsyncInventoryContents: a request id at once, the contents to the reply target.
pub async fn async_contents(ctx: &Ctx, folder: &str, target: &str) -> Answer {
    const C: &str = "AsyncInventoryContents";
    let Some(id) = key(folder) else {
        return failure(C, "Invaild folder UUID", &owned(&[folder]));
    };
    let request = uuid::Uuid::new_v4().to_string();
    let (ctx2, folder2, target2, req) = (
        ctx.clone(),
        folder.to_owned(),
        target.to_owned(),
        request.clone(),
    );
    tokio::spawn(async move {
        let data = contents_json(&ctx2, id).await;
        let r = AsyncReply {
            request: req,
            status: true,
            message: folder2,
            data,
        };
        async_reply(&ctx2, &target2, C, r).await;
    });
    basic(C, &request, &owned(&[folder, target]))
}

/// AsyncSendItem: a request id at once; the item's key, or why it was not sent, to the reply
/// target.
pub async fn async_send_item(ctx: &Ctx, item: &str, who: &str, target: &str) -> Answer {
    const C: &str = "AsyncSendItem";
    let Some(to) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar uuid", &owned(&[item, who]));
    };
    let Some(id) = key(item) else {
        return failure(C, "Invaild item uuid", &owned(&[item, who]));
    };
    let request = uuid::Uuid::new_v4().to_string();
    let (ctx2, item2, target2, req) = (
        ctx.clone(),
        item.to_owned(),
        target.to_owned(),
        request.clone(),
    );
    tokio::spawn(async move {
        let found = ctx2.client.inventory_item(id).await.ok().flatten();
        let (status, data) = match found {
            None => (false, "Unable to find item".to_owned()),
            Some(i) => {
                let t = u8::try_from(i.asset_type).unwrap_or(0);
                match ctx2.client.give_item_as(to, i.id, &i.name, t).await {
                    Ok(()) => (true, i.id.to_string()),
                    Err(e) => (false, e.to_string()),
                }
            }
        };
        let r = AsyncReply {
            request: req,
            status,
            message: item2,
            data,
        };
        async_reply(&ctx2, &target2, C, r).await;
    });
    basic(C, &request, &owned(&[item, who, target]))
}

/// AsyncSendItemByPath: as AsyncSendItem, the item found by its path.
pub async fn async_send_item_by_path(ctx: &Ctx, path: &str, who: &str, target: &str) -> Answer {
    const C: &str = "AsyncSendItemByPath";
    let Some(to) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar uuid", &owned(&[path, who]));
    };
    let request = uuid::Uuid::new_v4().to_string();
    let (ctx2, path2, target2, req) = (
        ctx.clone(),
        path.to_owned(),
        target.to_owned(),
        request.clone(),
    );
    tokio::spawn(async move {
        let (status, data) = match ctx2.client.item_by_path(&path2).await {
            Ok(Some(i)) => {
                let t = u8::try_from(i.asset_type).unwrap_or(0);
                match ctx2.client.give_item_as(to, i.id, &i.name, t).await {
                    Ok(()) => (true, i.id.to_string()),
                    Err(e) => (false, e.to_string()),
                }
            }
            _ => (false, "Unable to find item via path".to_owned()),
        };
        let r = AsyncReply {
            request: req,
            status,
            message: path2,
            data,
        };
        async_reply(&ctx2, &target2, C, r).await;
    });
    basic(C, &request, &owned(&[path, who, target]))
}

pub async fn send_item(ctx: &Ctx, item: &str, who: &str) -> Answer {
    const C: &str = "SendItem";
    let args = owned(&[item, who]);
    let Some(to) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar uuid", &args);
    };
    let Some(id) = key(item) else {
        return failure(C, "Invaild item uuid", &args);
    };
    match ctx.client.give_item(to, id).await {
        Ok(()) => basic(C, "ok", &args),
        Err(_) => failure(C, "Unable to find item", &args),
    }
}

pub async fn send_folder(ctx: &Ctx, folder: &str, who: &str) -> Answer {
    const C: &str = "SendFolder";
    let args = owned(&[folder, who]);
    let Some(to) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar uuid", &args);
    };
    let Some(id) = key(folder) else {
        return failure(C, "Invaild folder uuid", &args);
    };
    let name = ctx
        .client
        .folder_name(id)
        .unwrap_or_else(|| "Folder".into());
    match ctx.client.give_folder(to, id, &name).await {
        Ok(()) => basic(C, "ok", &args),
        Err(_) => failure(C, "Unable to find folder", &args),
    }
}

pub async fn send_item_by_path(ctx: &Ctx, path: &str, who: &str) -> Answer {
    const C: &str = "SendItemByPath";
    let args = owned(&[path, who]);
    let Some(to) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar uuid", &args);
    };
    let item = match ctx.client.item_by_path(path).await {
        Ok(Some(i)) => i,
        Ok(None) => return failure(C, "Unable to find item via path", &args),
        Err(e) => return failure(C, &e.to_string(), &args),
    };
    let t = u8::try_from(item.asset_type).unwrap_or(0);
    match ctx.client.give_item_as(to, item.id, &item.name, t).await {
        Ok(()) => basic(C, "ok", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

pub async fn send_folder_by_path(ctx: &Ctx, path: &str, who: &str) -> Answer {
    const C: &str = "SendFolderByPath";
    let args = owned(&[path, who]);
    let Some(to) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar uuid", &args);
    };
    let folder = match ctx.client.folder_by_path(path).await {
        Ok(Some(f)) => f,
        Ok(None) => return failure(C, "Unable to find folder", &args),
        Err(e) => return failure(C, &e.to_string(), &args),
    };
    match ctx.client.give_folder(to, folder.id, &folder.name).await {
        Ok(()) => basic(C, "ok", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

/// An asset type by LibreMetaverse's AssetType name (SecondBot matches those, lower case), or
/// by the viewer's short name, which SecondBot's own hints give (callcard, animatn, material).
fn asset_type(name: &str) -> Option<u8> {
    let n = name.trim().to_ascii_lowercase();
    let t = match n.as_str() {
        "callingcard" => 2,
        "script" => 4,
        "lslbytecode" => 11,
        "texturetga" => 12,
        "soundwav" => 17,
        "imagetga" => 18,
        "imagejpeg" => 19,
        "animation" => 20,
        "widget" => 40,
        "person" => 45,
        other => ferro_client::inventory::asset_type_from_name(other),
    };
    // folders are not items
    u8::try_from(t).ok().filter(|t| !matches!(t, 8 | 25))
}

pub async fn send_item_direct(ctx: &Ctx, item: &str, name: &str, kind: &str, who: &str) -> Answer {
    const C: &str = "SendItemDirect";
    let args = owned(&[item, name, kind, who]);
    let Some(id) = key(item) else {
        return failure(C, "Invaild item uuid", &args);
    };
    if name.trim().is_empty() || kind.trim().is_empty() {
        return failure(C, "Invaild item name", &args);
    }
    let Some(t) = asset_type(kind) else {
        return failure(C, "Invaild item type", &args);
    };
    let Some(to) = avatar(&ctx.client, who).await else {
        return failure(C, "Invaild avatar uuid", &args);
    };
    // SecondBot answers with the avatar's key in place of what was sent
    let args = owned(&[item, name, kind, &to.to_string()]);
    match ctx.client.give_item_as(to, id, name, t).await {
        Ok(()) => basic(C, "ok", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

pub async fn real_uuid(ctx: &Ctx, item: &str) -> Answer {
    const C: &str = "GetRealUUID";
    let args = owned(&[item]);
    let Some(id) = key(item) else {
        return failure(C, "Invaild item uuid", &args);
    };
    let asset = ctx
        .client
        .item_asset(id)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    basic(C, &asset.to_string(), &args)
}

pub async fn create_folder(ctx: &Ctx, parent: &str, name: &str) -> Answer {
    const C: &str = "CreateInventoryFolder";
    let Some(parent_id) = key(parent) else {
        return failure(C, "invaild parent folder UUID", &owned(&[parent]));
    };
    if name.chars().count() < 3 {
        return failure(
            C,
            "new folder name is too short, must be longer than 3 characters.",
            &owned(&[name]),
        );
    }
    match ctx.client.create_folder(parent_id, name).await {
        Ok(id) => basic(C, "Ok", &[id.to_string()]),
        Err(e) => failure(C, &e.to_string(), &owned(&[parent, name])),
    }
}

pub async fn rename(ctx: &Ctx, item: &str, name: &str) -> Answer {
    const C: &str = "RenameInventory";
    let args = owned(&[item, name]);
    let Some(id) = key(item) else {
        return failure(C, "invaild item uuid", &args);
    };
    if name.chars().count() < 3 {
        return failure(C, "Item name is to short", &args);
    }
    match ctx.client.rename_item(id, name).await {
        Ok(()) => basic(C, "Ok", &args),
        Err(_) => failure(C, "Unable to find inventory item", &args),
    }
}

pub async fn delete_item(ctx: &Ctx, item: &str) -> Answer {
    const C: &str = "DeleteInventoryItem";
    let args = owned(&[item]);
    let Some(id) = key(item) else {
        return failure(C, "invaild item uuid", &args);
    };
    match ctx.client.trash_item(id).await {
        Ok(()) => basic(C, "Ok", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

pub async fn delete_folder(ctx: &Ctx, folder: &str) -> Answer {
    const C: &str = "DeleteInventoryFolder";
    let args = owned(&[folder]);
    let Some(id) = key(folder) else {
        return failure(C, "invaild folder uuid", &args);
    };
    match ctx.client.trash_folder_of(id).await {
        Ok(()) => basic(C, "Ok", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

pub async fn move_item(ctx: &Ctx, item: &str, folder: &str) -> Answer {
    const C: &str = "MoveInventoryItem";
    let args = owned(&[item, folder]);
    let Some(id) = key(item) else {
        return failure(C, "invaild item uuid", &args);
    };
    let Some(to) = key(folder) else {
        return failure(C, "invaild folder uuid", &args);
    };
    match ctx.client.move_item(id, to, None).await {
        Ok(()) => basic(C, "Ok", &owned(&[item])),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

pub async fn move_folder(ctx: &Ctx, folder: &str, dest: &str) -> Answer {
    const C: &str = "MoveInventoryFolder";
    let args = owned(&[folder, dest]);
    let Some(id) = key(folder) else {
        return failure(C, "invaild source folder uuid", &args);
    };
    let Some(to) = key(dest) else {
        return failure(C, "invaild dest folder uuid", &args);
    };
    match ctx.client.move_folder(id, to).await {
        Ok(()) => basic(C, "Ok", &args),
        Err(e) => failure(C, &e.to_string(), &args),
    }
}

/// How old a notecard InventoryPurgeNotecards removes is, at least.
const PURGE_AGE_DAYS: i64 = 31;

/// InventoryPurgeNotecards: the notecards of the root's "Notecards" folder made 31 days ago or
/// more, removed for good.
pub async fn purge_notecards(ctx: &Ctx) -> Answer {
    const C: &str = "InventoryPurgeNotecards";
    let root = ctx.client.inventory_root();
    let folder = match ctx.client.folder_contents(root).await {
        Ok(c) => c.folders.into_iter().find(|f| f.name == "Notecards"),
        Err(e) => {
            log::warn!("{C}: {e}");
            None
        }
    };
    let Some(folder) = folder else {
        return failure(C, "Unable to find notecard folder", &[]);
    };
    let contents = match ctx.client.folder_contents(folder.id).await {
        Ok(c) => c,
        Err(e) => return failure(C, &e.to_string(), &[]),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let old: Vec<uuid::Uuid> = contents
        .items
        .iter()
        .filter(|i| i.asset_type == 7 && (now - i.created_at) / 86_400 >= PURGE_AGE_DAYS)
        .map(|i| i.id)
        .collect();
    if !old.is_empty() {
        log::info!("{C}: removing {} notecards", old.len());
        if let Err(e) = ctx.client.remove_items(old).await {
            return failure(C, &e.to_string(), &[]);
        }
    }
    basic(C, "Ok", &[])
}

/// SecondBot's InventoryNodeEntry, its fields in their order; an item has no children (null).
#[derive(Serialize)]
struct Node {
    #[serde(rename = "UUID")]
    id: String,
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Children")]
    children: Option<Vec<Node>>,
    #[serde(rename = "InventoryType")]
    kind: String,
}

/// Folders per fetch, walking the whole tree.
const FOLDERS_PER_FETCH: usize = 10;

/// InventoryContentsCatched: the whole inventory as a tree, from "My Inventory" down.
pub async fn contents_catched(ctx: &Ctx) -> Answer {
    const C: &str = "InventoryContentsCatched";
    let root = ctx.client.inventory_root();
    let mut fetched = std::collections::HashMap::new();
    let mut todo = vec![root];
    while !todo.is_empty() {
        let batch: Vec<uuid::Uuid> = todo.drain(..todo.len().min(FOLDERS_PER_FETCH)).collect();
        match ctx.client.fetch_folders(&batch).await {
            Ok(found) => {
                for c in found {
                    for f in &c.folders {
                        if !fetched.contains_key(&f.id) && !todo.contains(&f.id) {
                            todo.push(f.id);
                        }
                    }
                    fetched.insert(c.folder_id, c);
                }
            }
            Err(e) => {
                log::warn!("{C}: {e}");
                if fetched.is_empty() {
                    return failure(C, &e.to_string(), &[]);
                }
            }
        }
    }
    fn node(
        id: uuid::Uuid,
        name: &str,
        fetched: &std::collections::HashMap<uuid::Uuid, ferro_client::inventory::FolderContents>,
        depth: usize,
    ) -> Node {
        let mut children = Vec::new();
        // a folder inside itself would never end
        if let Some(c) = fetched.get(&id).filter(|_| depth < 64) {
            children.extend(
                c.folders
                    .iter()
                    .map(|f| node(f.id, &f.name, fetched, depth + 1)),
            );
            children.extend(c.items.iter().map(|i| Node {
                id: i.id.to_string(),
                name: i.name.clone(),
                children: None,
                kind: typename(i.inv_type).trim_start_matches("Inventory").into(),
            }));
        }
        Node {
            id: id.to_string(),
            name: name.to_owned(),
            children: Some(children),
            kind: "Folder".into(),
        }
    }
    basic(C, &json(&node(root, "My Inventory", &fetched, 0)), &[])
}

pub fn preload_count(ctx: &Ctx) -> Answer {
    let (folders, items) = ctx.client.inventory_seen();
    basic(
        "InventoryPreloadCount",
        &format!("folders:{folders},items:{items}"),
        &[],
    )
}

pub fn preload_done() -> Answer {
    basic("InventoryPreloadDone", "Ready", &[])
}

/// SetInventoryUpdate: the target hears of each item of this type that comes into the
/// inventory (crate::inventory_updates).
pub fn set_inventory_update(ctx: &Ctx, kind: &str, target: &str) -> Answer {
    use crate::inventory_updates::{asset_type_name, event_type_name};
    let name = asset_type(kind)
        .and_then(|t| asset_type_name(i32::from(t)))
        .map_or_else(|| event_type_name(kind), event_type_name);
    ctx.inventory_updates.add(&name, target.trim());
    basic("SetInventoryUpdate", "Ok", &[])
}

/// The largest sound UploadMediaWave downloads: 30 s of mono Ogg Vorbis is far smaller, even at
/// the encoder's highest quality.
const MAX_SOUND_FILE: usize = 2 * 1024 * 1024;

/// UploadMediaWave: a sound from an http(s) URL into the Sounds folder, as `inventoryName`.
pub async fn upload_media_wave(ctx: &Ctx, source: &str, name: &str) -> Answer {
    const C: &str = "UploadMediaWave";
    let args = owned(&[source, name]);
    let price = ctx.client.state().sound_upload_cost;
    if !ctx.allow_funds && price != Some(0) {
        return failure(C, "Funds commands are disabled", &args);
    }
    if name.trim().is_empty() || name.len() > 63 {
        return failure(C, "inventoryName must be 1 to 63 bytes", &args);
    }
    if !crate::download::allowed(source) {
        return failure(C, "sourcePath must start with http:// or https://", &args);
    }
    let data = match crate::download::fetch(source, MAX_SOUND_FILE).await {
        Ok(data) => data,
        Err(e) => {
            let why = format!("Unable to download the sound from url error {e}");
            return failure(C, &why, &args);
        }
    };
    let sound = match ferro_client::sound::check(&data) {
        Ok(sound) => sound,
        Err(why) => return failure(C, &format!("Not an Ogg Vorbis sound: {why}"), &args),
    };
    if let Some(price) = price.filter(|p| *p > 0) {
        match ctx.client.balance().await {
            Ok(balance) if balance >= price => {}
            Ok(_) => {
                let why = format!("Not enough funds (L${price} needed)");
                return failure(C, &why, &args);
            }
            Err(e) => {
                log::debug!("{C}: the balance: {e}");
                return failure(C, "No reply from the grid [Retry later]", &args);
            }
        }
    }
    match ctx.client.upload_sound(name.trim(), data, price).await {
        Ok(done) => {
            log::info!(
                "{C}: \"{}\" uploaded ({:.1} s), item {}, L${}",
                name.trim(),
                sound.seconds(),
                done.item,
                done.cost
            );
            basic(C, "ok", &[])
        }
        Err(e) => failure(C, &format!("Upload failed: {e}"), &args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_types_by_either_name() {
        assert_eq!(asset_type("Object"), Some(6));
        assert_eq!(asset_type("notecard"), Some(7));
        assert_eq!(asset_type("CallingCard"), Some(2));
        assert_eq!(asset_type("callcard"), Some(2));
        assert_eq!(asset_type("animation"), Some(20));
        assert_eq!(asset_type("animatn"), Some(20));
        assert_eq!(asset_type("folder"), None);
        assert_eq!(asset_type("category"), None);
        assert_eq!(asset_type("thing"), None);
        assert_eq!(asset_type("lslbyte"), Some(11));
        assert_eq!(asset_type("Lsltext"), Some(10));
        assert_eq!(asset_type("Mesh"), Some(49));
    }

    #[test]
    fn items_in_the_tree_have_null_children() {
        let n = Node {
            id: "a".into(),
            name: "Note".into(),
            children: None,
            kind: typename(7).trim_start_matches("Inventory").into(),
        };
        assert_eq!(
            json(&n),
            r#"{"UUID":"a","Name":"Note","Children":null,"InventoryType":"Notecard"}"#
        );
    }
}
