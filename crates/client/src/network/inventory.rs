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
}

impl Session<'_> {
    pub(super) fn on_inventory_request(&mut self, r: InventoryRequest) {
        let (agent, session) = (self.login.agent_id, self.login.session_id);
        match r {
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
