//! The mute list: its download and its file.
//!
//! Port of the mute list Xfer of aurora-net/src/session_social.rs and of parse_line in
//! aurora-viewer/src/world/mutes.rs (Aurora Viewer, see NOTICE.md): MuteListRequest, then
//! MuteListUpdate names a file the region sends by Xfer (LLMuteList, LLXferManager, originally
//! LGPL 2.1); "emptymutelist" when the resident blocks nobody.

use uuid::Uuid;

/// ELLPath LL_PATH_CACHE: where the region writes the mute list file.
pub(crate) const LL_PATH_CACHE: u8 = 4;
/// The last packet of an xfer (LLXferManager::encodePacketNum).
pub(crate) const LAST_PACKET: u32 = 0x8000_0000;
/// LL_XFER_LARGE_PAYLOAD and the size before the first packet's data.
const MAX_XFER_PACKET: usize = 7680 + 4;
/// The largest mute list taken (MuteListLimit, 1000 entries of about 300 bytes).
const MAX_FILE: usize = 1 << 20;
/// GenericMessage method of the answer for an empty list (LLDispatchEmptyMuteList).
pub(crate) const EMPTY_MUTE_LIST: &str = "emptymutelist";

/// One entry (LLMute).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mute {
    /// LLMute::EType: 0 by name, 1 an avatar, 2 an object, 3 a group, 4 external.
    pub kind: i32,
    pub id: Uuid,
    pub name: String,
    /// LLMute flags: what is not muted (1 text chat, 2 voice, 4 particles, 8 object sounds).
    pub flags: u32,
}

/// A line of the file: `type id name|flags` (LLMuteList::loadFromFile).
fn parse_line(line: &str) -> Option<Mute> {
    let line = line.trim_start();
    let (kind, rest) = line.split_once(char::is_whitespace)?;
    let kind: i32 = kind.parse().ok()?;
    let rest = rest.trim_start();
    let (id, rest) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let id = Uuid::parse_str(id).unwrap_or_default();
    let rest = rest.trim_start();
    let (name, flags) = match rest.split_once('|') {
        Some((n, f)) => (n, f.trim().parse::<u32>().unwrap_or(0)),
        None => (rest.trim_end(), 0),
    };
    Some(Mute {
        kind,
        id,
        name: name.to_owned(),
        flags,
    })
}

/// The file's entries; lines that do not read are left out.
pub fn parse_file(text: &str) -> Vec<Mute> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(parse_line)
        .collect()
}

/// A file download under way (LLXfer_File, the receiving side).
#[derive(Debug)]
pub(crate) struct XferDownload {
    pub id: u64,
    /// The packet expected next.
    next: u32,
    size: usize,
    data: Vec<u8>,
}

/// What a SendXferPacket means for the download.
#[derive(Debug, PartialEq)]
pub(crate) enum Step {
    /// Not ours.
    Ignore,
    /// Confirm this packet (ConfirmXferPacket), and wait for more.
    Confirm(u32),
    /// Confirm it: the file is complete.
    Done(u32, String),
    /// Give up (too large, or a first packet without its size).
    Fail(&'static str),
}

impl XferDownload {
    pub fn new(id: u64) -> XferDownload {
        XferDownload {
            id,
            next: 0,
            size: 0,
            data: Vec::new(),
        }
    }

    /// LLXferManager::processReceiveData.
    pub fn packet(&mut self, id: u64, packet: u32, data: &[u8]) -> Step {
        if id != self.id {
            return Step::Ignore;
        }
        let num = packet & !LAST_PACKET;
        if num != self.next {
            // the last packet again: its confirmation was lost
            return if num + 1 == self.next {
                Step::Confirm(num)
            } else {
                Step::Ignore
            };
        }
        let data = &data[..data.len().min(MAX_XFER_PACKET)];
        if num == 0 {
            // the first packet: the S32 size first, little-endian
            let Some(size) = data.get(..4) else {
                return Step::Fail("a first packet without its size");
            };
            self.size = i32::from_le_bytes([size[0], size[1], size[2], size[3]]).max(0) as usize;
            self.data.extend_from_slice(&data[4..]);
        } else {
            self.data.extend_from_slice(data);
        }
        self.next += 1;
        if self.data.len() > MAX_FILE {
            return Step::Fail("the mute list is too large");
        }
        if packet & LAST_PACKET == 0 {
            return Step::Confirm(num);
        }
        if self.size != self.data.len() {
            log::warn!(
                "mute list: {} bytes received, {} announced",
                self.data.len(),
                self.size
            );
        }
        Step::Done(num, String::from_utf8_lossy(&self.data).into_owned())
    }
}

/// Xfer ids only need to be unique among the bot's transfers.
pub(crate) fn xfer_id() -> u64 {
    let b = Uuid::new_v4().into_bytes();
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_reads_as_the_viewer_reads_it() {
        let a = Uuid::from_u128(1);
        let text = format!(
            "0 {} Spammer Bob|\n1 {a} Jane Doe|0\n\n2 {} Noisy Box|3\nnot a line\n",
            Uuid::nil(),
            Uuid::nil()
        );
        let list = parse_file(&text);
        assert_eq!(list.len(), 3);
        assert_eq!(
            list[1],
            Mute {
                kind: 1,
                id: a,
                name: "Jane Doe".into(),
                flags: 0
            }
        );
        assert_eq!((list[2].name.as_str(), list[2].flags), ("Noisy Box", 3));
        assert_eq!(list[0].name, "Spammer Bob");
    }

    #[test]
    fn packets_make_the_file() {
        let mut x = XferDownload::new(7);
        assert_eq!(x.packet(8, 0, b"xx"), Step::Ignore);
        let mut first = 9i32.to_le_bytes().to_vec();
        first.extend_from_slice(b"0 a");
        assert_eq!(x.packet(7, 0, &first), Step::Confirm(0));
        // the first again: its confirmation was lost
        assert_eq!(x.packet(7, 0, &first), Step::Confirm(0));
        assert_eq!(
            x.packet(7, 1 | LAST_PACKET, b" Bob|\n"),
            Step::Done(1, "0 a Bob|\n".into())
        );
        let mut x = XferDownload::new(7);
        assert!(matches!(x.packet(7, 0, b"ab"), Step::Fail(_)));
    }
}
