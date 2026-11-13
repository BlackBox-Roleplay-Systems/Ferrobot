//! The session's side of objects: what the region told of them (objects::Places), clicking one
//! (ObjectGrab then ObjectDeGrab, as LLToolGrab's click), rezzing from the inventory (RezObject,
//! LLToolDragAndDrop::dropObject, Aurora's inventory/rez.rs), taking one back (DeRezObject) and
//! putting an item in one (UpdateTaskInventory, RezScript for a script: dropInventory).

use super::Session;
use crate::inventory::InvItem;
use crate::objects::{Known, Root};
use ferro_msg::msgs::*;
use ferro_msg::str_field;
use glam::Vec3;
use tokio::sync::oneshot;
use uuid::Uuid;

/// DeRezObject's DeRezDestination: take into the agent's inventory.
const TAKE_INTO_AGENT_INVENTORY: u8 = 4;

#[derive(Debug)]
pub(crate) enum ObjectRequest {
    Find {
        key: Uuid,
        reply: oneshot::Sender<Option<Known>>,
    },
    Roots {
        reply: oneshot::Sender<Vec<Root>>,
    },
    Click {
        local_id: u32,
        reply: oneshot::Sender<()>,
    },
    /// Rez an item of the bot's at a place in the region; answers the query id.
    Rez {
        item: Box<InvItem>,
        at: Vec3,
        reply: oneshot::Sender<Uuid>,
    },
    /// Take an object back into a folder of the bot's.
    DeRez {
        local_id: u32,
        folder: Uuid,
        reply: oneshot::Sender<()>,
    },
    /// Put an item in an object; a script running or not.
    ToTask {
        local_id: u32,
        item: Box<InvItem>,
        running: bool,
        reply: oneshot::Sender<()>,
    },
}

/// LLInventoryItem's fields as RezObject, UpdateTaskInventory and RezScript carry them
/// (packMessage): the item in `folder`, its CRC with it.
macro_rules! item_block {
    ($b:expr, $i:expr, $folder:expr, $transaction:expr) => {{
        let i: &InvItem = $i;
        $b.item_id = i.id;
        $b.folder_id = $folder;
        $b.creator_id = i.creator;
        $b.owner_id = i.owner;
        $b.group_id = i.group_id;
        $b.base_mask = i.base_mask;
        $b.owner_mask = i.owner_mask;
        $b.group_mask = i.group_mask;
        $b.everyone_mask = i.everyone_mask;
        $b.next_owner_mask = i.next_owner_mask;
        $b.group_owned = i.group_owned;
        $b.transaction_id = $transaction;
        $b.type_ = i.asset_type as i8;
        $b.inv_type = i.inv_type as i8;
        $b.flags = i.flags;
        $b.sale_type = i.sale_type;
        $b.sale_price = i.sale_price;
        $b.name = str_field(&i.name);
        $b.description = str_field(&i.desc);
        $b.creation_date = i.created_at as i32;
        $b.crc = crate::inventory::checksum_in_folder(i, $folder);
    }};
}

impl Session<'_> {
    pub(super) fn on_object_request(&mut self, r: ObjectRequest) {
        let (agent, session) = (self.login.agent_id, self.login.session_id);
        let group = self
            .shared
            .state()
            .active_group
            .as_ref()
            .map_or(Uuid::nil(), |g| g.id);
        match r {
            ObjectRequest::Find { key, reply } => {
                let _ = reply.send(self.places.find(key));
            }
            ObjectRequest::Roots { reply } => {
                let _ = reply.send(self.places.roots());
            }
            ObjectRequest::Click { local_id, reply } => {
                // a click is a grab at once let go (LLToolGrab)
                let mut g = ObjectGrab::default();
                g.agent_data.agent_id = agent;
                g.agent_data.session_id = session;
                g.object_data.local_id = local_id;
                self.send(&g, true);
                let mut d = ObjectDeGrab::default();
                d.agent_data.agent_id = agent;
                d.agent_data.session_id = session;
                d.object_data.local_id = local_id;
                self.send(&d, true);
                let _ = reply.send(());
            }
            ObjectRequest::Rez { item, at, reply } => {
                // on the land at a place: no ray cast (LibreMetaverse's RequestRezFromInventory)
                let query = Uuid::new_v4();
                let mut m = RezObject::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.agent_data.group_id = group;
                let r = &mut m.rez_data;
                r.bypass_raycast = 1;
                r.ray_start = at;
                r.ray_end = at;
                r.ray_end_is_intersection = false;
                r.rez_selected = false;
                r.remove_item = true;
                r.item_flags = item.flags;
                r.group_mask = item.group_mask;
                r.everyone_mask = item.everyone_mask;
                r.next_owner_mask = item.next_owner_mask;
                item_block!(m.inventory_data, &item, item.parent, query);
                self.send(&m, true);
                let _ = reply.send(query);
            }
            ObjectRequest::DeRez {
                local_id,
                folder,
                reply,
            } => {
                let mut m = DeRezObject::default();
                m.agent_data.agent_id = agent;
                m.agent_data.session_id = session;
                m.agent_block.group_id = group;
                m.agent_block.destination = TAKE_INTO_AGENT_INVENTORY;
                m.agent_block.destination_id = folder;
                m.agent_block.transaction_id = Uuid::new_v4();
                m.agent_block.packet_count = 1;
                m.agent_block.packet_number = 0;
                m.object_data = vec![de_rez_object::ObjectData {
                    object_local_id: local_id,
                }];
                self.send(&m, true);
                let _ = reply.send(());
            }
            ObjectRequest::ToTask {
                local_id,
                item,
                running,
                reply,
            } => {
                if item.asset_type == 10 {
                    // LLToolDragAndDrop::dropScript
                    let mut m = RezScript::default();
                    m.agent_data.agent_id = agent;
                    m.agent_data.session_id = session;
                    m.agent_data.group_id = group;
                    m.update_block.object_local_id = local_id;
                    m.update_block.enabled = running;
                    item_block!(m.inventory_block, &item, item.parent, Uuid::nil());
                    self.send(&m, true);
                } else {
                    // dropInventory: UpdateTaskInventory with the item key (UpdateData Key 0)
                    let mut m = UpdateTaskInventory::default();
                    m.agent_data.agent_id = agent;
                    m.agent_data.session_id = session;
                    m.update_data.local_id = local_id;
                    m.update_data.key = 0;
                    item_block!(m.inventory_data, &item, item.parent, Uuid::nil());
                    self.send(&m, true);
                }
                let _ = reply.send(());
            }
        }
    }
}
