//! Port of aurora-net/src/session/inventory_preview.rs (Aurora Viewer, see NOTICE.md).
//!
//! Protected asset downloads (notecards, scripts): LLAssetStorage::getInvItemAsset,
//! LLTransferSourceParamsInvItem and LLTransferTarget (originally LGPL 2.1). ViewerAsset cannot
//! read these assets by key alone: the region checks the item's owner and permissions, then
//! sends the asset over UDP (TransferInfo, then numbered TransferPackets).

use crate::inventory::InvItem;
use ferro_msg::msgs::TransferRequest;
use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;
use uuid::Uuid;

pub(crate) const ASSET_CHANNEL: i32 = 2;
const INVENTORY_SOURCE: i32 = 3;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_PACKETS: i32 = 8192;
const MAX_DOWNLOADS: usize = 32;
const TIMEOUT: Duration = Duration::from_secs(30);

/// What the region sends about a transfer.
#[derive(Debug)]
pub(crate) enum Reply {
    Info {
        status: i32,
        size: i32,
    },
    Packet {
        status: i32,
        packet: i32,
        data: Vec<u8>,
    },
    Abort,
}

fn status_error(status: i32) -> String {
    // LLTS_INSUFFICIENT_PERMISSIONS
    if status == -3 {
        "not allowed to read it".into()
    } else {
        format!("the transfer failed (status {status})")
    }
}

const INTERRUPTED: &str = "the transfer was interrupted";
const TOO_LARGE: &str = "too large";

/// Waits for TransferInfo and every numbered packet. A DONE packet can arrive before packet zero
/// or TransferInfo; it never makes a truncated file valid.
#[derive(Default)]
struct Assembly {
    size: Option<usize>,
    packets: BTreeMap<i32, Vec<u8>>,
    received: usize,
    last: Option<i32>,
}

impl Assembly {
    fn accept(&mut self, reply: Reply) -> Result<Option<Vec<u8>>, String> {
        match reply {
            Reply::Info { status, size } => {
                if status != 0 {
                    return Err(status_error(status));
                }
                let size = usize::try_from(size).map_err(|_| INTERRUPTED.to_owned())?;
                if size > MAX_BYTES {
                    return Err(TOO_LARGE.into());
                }
                if self.size.is_some_and(|old| old != size) {
                    return Err(INTERRUPTED.into());
                }
                self.size = Some(size);
            }
            Reply::Packet {
                status,
                packet,
                data,
            } => {
                if !matches!(status, 0 | 1) {
                    return Err(status_error(status));
                }
                if !(0..MAX_PACKETS).contains(&packet) || self.last.is_some_and(|l| packet > l) {
                    return Err(INTERRUPTED.into());
                }
                if status == 1 {
                    if self.last.is_some_and(|l| l != packet)
                        || self.packets.keys().any(|p| *p > packet)
                    {
                        return Err(INTERRUPTED.into());
                    }
                    self.last = Some(packet);
                }
                if let Some(old) = self.packets.get(&packet) {
                    if *old != data {
                        return Err(INTERRUPTED.into());
                    }
                } else {
                    if self.received + data.len() > MAX_BYTES {
                        return Err(TOO_LARGE.into());
                    }
                    self.received += data.len();
                    self.packets.insert(packet, data);
                }
            }
            Reply::Abort => return Err(INTERRUPTED.into()),
        }
        let Some(size) = self.size else {
            return Ok(None);
        };
        if self.received > size {
            return Err(INTERRUPTED.into());
        }
        let complete = self
            .last
            .is_some_and(|l| self.packets.len() == l as usize + 1)
            && self
                .packets
                .keys()
                .copied()
                .eq(0..self.packets.len() as i32);
        if !complete {
            return Ok(None);
        }
        if self.received != size {
            return Err(INTERRUPTED.into());
        }
        Ok(Some(
            std::mem::take(&mut self.packets)
                .into_values()
                .flatten()
                .collect(),
        ))
    }
}

/// LLDataPackerBinaryBuffer: six keys (agent, session, owner, task, item, asset), then the S32
/// asset type, little-endian. `object` is nil for the bot's own inventory.
pub(crate) fn request(agent: Uuid, session: Uuid, object: Uuid, item: &InvItem) -> TransferRequest {
    let mut m = TransferRequest::default();
    let t = &mut m.transfer_info;
    t.transfer_id = Uuid::new_v4();
    t.channel_type = ASSET_CHANNEL;
    t.source_type = INVENTORY_SOURCE;
    // getInvItemAsset asks at priority 100 + 1
    t.priority = 101.0;
    for id in [agent, session, item.owner, object, item.id, item.asset_id] {
        t.params.extend_from_slice(id.as_bytes());
    }
    t.params.extend_from_slice(&item.asset_type.to_le_bytes());
    m
}

struct Download {
    started: Instant,
    data: Assembly,
    reply: oneshot::Sender<Result<Vec<u8>, String>>,
}

/// The downloads under way, by transfer id.
#[derive(Default)]
pub(crate) struct Transfers {
    pending: HashMap<Uuid, Download>,
}

impl Transfers {
    /// A new download; false (and the caller answered) when too many are under way.
    pub fn start(
        &mut self,
        transfer: Uuid,
        reply: oneshot::Sender<Result<Vec<u8>, String>>,
    ) -> bool {
        if self.pending.len() >= MAX_DOWNLOADS {
            let _ = reply.send(Err("too many downloads under way".into()));
            return false;
        }
        self.pending.insert(
            transfer,
            Download {
                started: Instant::now(),
                data: Assembly::default(),
                reply,
            },
        );
        true
    }

    /// A reply from the region. Answers whether it was one of ours, and whether to abort it
    /// (it failed).
    pub fn receive(&mut self, transfer: Uuid, channel: i32, reply: Reply) -> (bool, bool) {
        let Some(d) = self
            .pending
            .get_mut(&transfer)
            .filter(|_| channel == ASSET_CHANNEL)
        else {
            return (false, false);
        };
        let result = match d.data.accept(reply) {
            Ok(None) => return (true, false),
            Ok(Some(data)) => Ok(data),
            Err(why) => Err(why),
        };
        let failed = result.is_err();
        if let Some(d) = self.pending.remove(&transfer) {
            let _ = d.reply.send(result);
        }
        (true, failed)
    }

    /// The downloads that took too long, or whose caller gave up: answered and forgotten. Their
    /// ids go back, to be aborted.
    pub fn expire(&mut self) -> Vec<Uuid> {
        let gone: Vec<Uuid> = self
            .pending
            .iter()
            .filter(|(_, d)| d.started.elapsed() >= TIMEOUT || d.reply.is_closed())
            .map(|(id, _)| *id)
            .collect();
        for id in &gone {
            if let Some(d) = self.pending.remove(id) {
                let _ = d.reply.send(Err("the download timed out".into()));
            }
        }
        gone
    }

    /// The region changed: what was asked of the old one will not come.
    pub fn clear(&mut self) {
        for (_, d) in self.pending.drain() {
            let _ = d.reply.send(Err(INTERRUPTED.into()));
        }
    }
}

/// The text of a notecard asset (LLNotecard::importStream: "Linden text version N {
/// LLEmbeddedItems ... Text length L <text> }", as Aurora's land.rs notecard_text). Each embedded
/// inventory character (the private use area) becomes `embedded`.
pub fn notecard_text(data: &[u8], embedded: &str) -> Option<String> {
    let s = String::from_utf8_lossy(data);
    if !s.starts_with("Linden text version") {
        return None;
    }
    let key = "Text length ";
    let at = s.find(key)? + key.len();
    let rest = &s[at..];
    let nl = rest.find('\n')?;
    let len: usize = rest[..nl].trim().parse().ok()?;
    let body = &rest.as_bytes()[nl + 1..];
    let text = String::from_utf8_lossy(&body[..len.min(body.len())]).into_owned();
    Some(
        text.chars()
            .map(|c| {
                if ('\u{F0000}'..='\u{FFFFD}').contains(&c) {
                    embedded.to_owned()
                } else {
                    c.to_string()
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(packet: i32, data: &[u8], last: bool) -> Reply {
        Reply::Packet {
            status: i32::from(last),
            packet,
            data: data.to_vec(),
        }
    }

    #[test]
    fn the_request_carries_the_item_and_its_owner() {
        let item = InvItem {
            owner: Uuid::from_u128(3),
            id: Uuid::from_u128(4),
            asset_id: Uuid::from_u128(5),
            asset_type: 7,
            ..Default::default()
        };
        let m = request(Uuid::from_u128(1), Uuid::from_u128(2), Uuid::nil(), &item);
        let t = &m.transfer_info;
        assert_eq!((t.channel_type, t.source_type), (2, 3));
        assert_eq!(t.priority, 101.0);
        let ids: Vec<Uuid> = t.params[..96]
            .chunks(16)
            .map(|b| Uuid::from_slice(b).expect("a key"))
            .collect();
        assert_eq!(
            ids,
            [
                Uuid::from_u128(1),
                Uuid::from_u128(2),
                item.owner,
                Uuid::nil(),
                item.id,
                item.asset_id
            ]
        );
        assert_eq!(&t.params[96..], &7i32.to_le_bytes());
    }

    #[test]
    fn packets_in_any_order_wait_for_the_size_and_all_of_them() {
        let mut a = Assembly::default();
        assert!(a.accept(packet(2, b"ghi", true)).expect("last").is_none());
        assert!(a.accept(packet(0, b"abc", false)).expect("first").is_none());
        assert!(a.accept(packet(0, b"abc", false)).expect("again").is_none());
        assert!(
            a.accept(packet(1, b"def", false))
                .expect("middle")
                .is_none()
        );
        assert_eq!(
            a.accept(Reply::Info { status: 0, size: 9 }).expect("info"),
            Some(b"abcdefghi".to_vec())
        );
    }

    #[test]
    fn bad_transfers_never_make_a_document() {
        let mut a = Assembly::default();
        a.accept(Reply::Info { status: 0, size: 7 }).expect("info");
        assert!(a.accept(packet(0, b"short", true)).is_err());
        assert!(
            Assembly::default()
                .accept(Reply::Info {
                    status: 0,
                    size: -1
                })
                .is_err()
        );
        assert_eq!(
            Assembly::default().accept(Reply::Info {
                status: -3,
                size: 0
            }),
            Err("not allowed to read it".into())
        );
        assert!(
            Assembly::default()
                .accept(packet(MAX_PACKETS, b"bad", false))
                .is_err()
        );
        let mut a = Assembly::default();
        a.accept(packet(0, b"first", false)).expect("first");
        assert!(a.accept(packet(0, b"other", false)).is_err());
    }

    #[test]
    fn replies_go_to_who_asked() {
        let mut t = Transfers::default();
        let (tx, mut rx) = oneshot::channel();
        let id = Uuid::from_u128(6);
        assert!(t.start(id, tx));
        assert_eq!(t.receive(Uuid::nil(), 2, Reply::Abort), (false, false));
        assert_eq!(t.receive(id, 1, Reply::Abort), (false, false));
        assert_eq!(
            t.receive(id, 2, Reply::Info { status: 0, size: 3 }),
            (true, false)
        );
        assert_eq!(t.receive(id, 2, packet(0, b"abc", true)), (true, false));
        assert_eq!(rx.try_recv().expect("answered"), Ok(b"abc".to_vec()));
        let (tx, mut rx) = oneshot::channel();
        t.start(id, tx);
        assert_eq!(t.receive(id, 2, Reply::Abort), (true, true));
        assert!(rx.try_recv().expect("answered").is_err());
    }

    #[test]
    fn notecard_text_and_its_embedded_items() {
        let card = "Linden text version 2\n{\nLLEmbeddedItems version 1\n{\ncount 0\n}\nText length 23\nBy purchasing\u{F0000} land\n}\n";
        assert_eq!(
            notecard_text(card.as_bytes(), "[ATTACHMENT]").as_deref(),
            Some("By purchasing[ATTACHMENT] land\n")
        );
        assert_eq!(notecard_text(b"not a notecard", ""), None);
    }
}
