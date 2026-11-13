//! SecondBot's Inventory commands that need no objects in world: the folders, a folder's
//! contents, sending items and folders, creating, renaming, moving and deleting.
//!
//! Deliberate differences: deleting moves to the Trash, as the viewer does (SecondBot removes for
//! good); InventoryFoldersLimited lists the folder's own subfolders (SecondBot's gives the folder
//! alone).

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
    basic(C, &json(&entries), &args)
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
    }
}
