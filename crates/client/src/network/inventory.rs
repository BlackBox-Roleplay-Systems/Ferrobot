//! The session's side of the inventory: the legacy fetches (FetchInventoryDescendents2,
//! FetchInventory2 capabilities) and the UDP changes LLInventoryModel sends (MoveInventoryItem,
//! MoveInventoryFolder, CreateInventoryFolder), and giving (an IM_INVENTORY_OFFERED).

use super::Session;
use crate::inventory::{self, FolderContents, InvItem};
use ferro_msg::msgs::*;
use ferro_msg::str_field;
use tokio::sync::oneshot;
use uuid::Uuid;

#[derive(Debug)]
pub(crate) enum InventoryRequest {
    Fetch {
        folders: Vec<Uuid>,
        reply: oneshot::Sender<Result<Vec<FolderContents>, String>>,
    },
    FetchItems {
        items: Vec<Uuid>,
        reply: oneshot::Sender<Result<Vec<InvItem>, String>>,
    },
    /// give_inventory: the bucket holds each entry's asset type and key, the first the one named.
    Give {
        to: Uuid,
        name: String,
        entries: Vec<(u8, Uuid)>,
        reply: oneshot::Sender<()>,
    },
    CreateFolder {
        id: Uuid,
        parent: Uuid,
        name: String,
        reply: oneshot::Sender<()>,
    },
    /// MoveInventoryItem: to a folder, renamed or not.
    MoveItem {
        item: Uuid,
        folder: Uuid,
        name: Option<String>,
        reply: oneshot::Sender<()>,
    },
    MoveFolder {
        folder: Uuid,
        parent: Uuid,
        reply: oneshot::Sender<()>,
    },
    /// CreateInventoryItem, answered by UpdateCreateInventoryItem with the new item's key.
    CreateItem {
        folder: Uuid,
        name: String,
        description: String,
        asset_type: i8,
        inv_type: i8,
        reply: oneshot::Sender<Uuid>,
    },
    /// A notecard's text saved (UpdateNotecardAgentInventory, then its uploader).
    UploadNotecard {
        item: Uuid,
        data: Vec<u8>,
        reply: oneshot::Sender<Result<Uuid, String>>,
    },
}

impl Session<'_> {
    pub(super) fn on_inventory_request(&mut self, r: InventoryRequest) {
        let (agent, session) = (self.login.agent_id, self.login.session_id);
        match r {
            InventoryRequest::CreateItem {
                folder,
                name,
                description,
                asset_type,
                inv_type,
                reply,
            } => {
                self.next_callback = self.next_callback.wrapping_add(1).max(1);
                let callback = self.next_callback;
                let mut m = CreateInventoryItem::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.inventory_block.callback_id = callback;
                m.inventory_block.folder_id = folder;
                m.inventory_block.next_owner_mask = 0x7fff_ffff;
                m.inventory_block.type_ = asset_type;
                m.inventory_block.inv_type = inv_type;
                m.inventory_block.name = str_field(&name);
                m.inventory_block.description = str_field(&description);
                self.send(&m, true);
                self.created.insert(callback, reply);
            }
            InventoryRequest::UploadNotecard { item, data, reply } => {
                let Some(cap) = self.caps.get("UpdateNotecardAgentInventory").cloned() else {
                    let _ = reply.send(Err("no UpdateNotecardAgentInventory capability".into()));
                    return;
                };
                let http = self.http.clone();
                tokio::spawn(async move {
                    let _ = reply.send(upload(&http, &cap, item, data).await);
                });
            }
            InventoryRequest::Fetch { folders, reply } => {
                let Some(url) = self.caps.get("FetchInventoryDescendents2").cloned() else {
                    let _ = reply.send(Err("no FetchInventoryDescendents2 capability".into()));
                    return;
                };
                let http = self.http.clone();
                tokio::spawn(async move {
                    let body = inventory::fetch_request_body(&folders, agent);
                    let answer = crate::caps::post_llsd(&http, &url, &body)
                        .await
                        .map(|v| inventory::parse_fetch_response(&v))
                        .map_err(|e| format!("FetchInventoryDescendents2: {e}"));
                    let _ = reply.send(answer);
                });
            }
            InventoryRequest::FetchItems { items, reply } => {
                let Some(url) = self.caps.get("FetchInventory2").cloned() else {
                    let _ = reply.send(Err("no FetchInventory2 capability".into()));
                    return;
                };
                let http = self.http.clone();
                tokio::spawn(async move {
                    let body = inventory::fetch_items_body(&items, agent);
                    let answer = crate::caps::post_llsd(&http, &url, &body)
                        .await
                        .map(|v| inventory::parse_items_response(&v))
                        .map_err(|e| format!("FetchInventory2: {e}"));
                    let _ = reply.send(answer);
                });
            }
            InventoryRequest::Give {
                to,
                name,
                entries,
                reply,
            } => {
                let bucket: Vec<u8> = entries
                    .iter()
                    .flat_map(|(t, id)| std::iter::once(*t).chain(id.as_bytes().iter().copied()))
                    .collect();
                // a new transaction for each offer (LLGiveInventory::doGiveInventoryItem)
                self.send_im(
                    to,
                    crate::events::dialog::INVENTORY_OFFERED,
                    Uuid::new_v4(),
                    &name,
                    bucket,
                );
                let _ = reply.send(());
            }
            InventoryRequest::CreateFolder {
                id,
                parent,
                name,
                reply,
            } => {
                let mut m = CreateInventoryFolder::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.folder_data.folder_id = id;
                m.folder_data.parent_id = parent;
                m.folder_data.type_ = -1;
                m.folder_data.name = str_field(&name);
                self.send(&m, true);
                let _ = reply.send(());
            }
            InventoryRequest::MoveItem {
                item,
                folder,
                name,
                reply,
            } => {
                let mut m = MoveInventoryItem::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.agent_data.stamp = false;
                m.inventory_data = vec![move_inventory_item::InventoryData {
                    item_id: item,
                    folder_id: folder,
                    new_name: name.map(|n| str_field(&n)).unwrap_or_default(),
                }];
                self.send(&m, true);
                let _ = reply.send(());
            }
            InventoryRequest::MoveFolder {
                folder,
                parent,
                reply,
            } => {
                let mut m = MoveInventoryFolder::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.agent_data.stamp = false;
                m.inventory_data = vec![move_inventory_folder::InventoryData {
                    folder_id: folder,
                    parent_id: parent,
                }];
                self.send(&m, true);
                let _ = reply.send(());
            }
        }
    }
}

/// LLPreviewNotecard::saveIfNeeded (Aurora's upload_content): the capability gives an uploader,
/// the asset goes to it, the answer names the new asset.
async fn upload(
    http: &reqwest::Client,
    cap: &str,
    item: Uuid,
    data: Vec<u8>,
) -> Result<Uuid, String> {
    let offer = crate::caps::post_llsd(http, cap, &ferro_llsd::llsd_map! { "item_id" => item })
        .await
        .map_err(|e| format!("the save was refused: {e}"))?;
    if offer["state"].as_str() != "upload" {
        return Err("the region offered no upload".into());
    }
    let url = offer["uploader"].to_string_value();
    let resp = http
        .post(url)
        .header("Content-Type", "application/octet-stream")
        .timeout(std::time::Duration::from_secs(30))
        .body(data)
        .send()
        .await
        .map_err(|e| format!("the upload failed: {}", e.without_url()))?;
    if !resp.status().is_success() {
        return Err(format!("the upload failed: HTTP {}", resp.status()));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| e.without_url().to_string())?;
    let ack = ferro_llsd::from_xml(&bytes).map_err(|_| "an unreadable answer".to_owned())?;
    if ack["state"].as_str() != "complete" {
        return Err("the save was not completed".into());
    }
    Ok(ack["new_asset"].as_uuid())
}
