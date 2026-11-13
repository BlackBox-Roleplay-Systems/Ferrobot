//! A terrain RAW file and its upload to the region by xfer.
//!
//! Port of the upload half of aurora-net/src/session/terrain_raw.rs and of `packet` in
//! aurora-net/src/session/inventory_upload.rs (Aurora Viewer, see NOTICE.md), which follow
//! LLPanelRegionTerrainInfo::onUploadRawFilepickerCB (newview/llfloaterregioninfo.cpp) and
//! LLXferManager / LLXfer_File (llmessage/llxfermanager.cpp, llxfer_file.cpp), originally
//! LGPL 2.1: the estate tools send `terrain` "upload filename" with the file's name, the region
//! asks for that name with RequestXfer, and the file goes in numbered SendXferPacket messages,
//! each sent once the region confirmed the one before (ConfirmXferPacket).

use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// A RAW file has 13 bytes per point of the region's height map (LLVLComposition's RAW: height,
/// height scale, then eleven channels the region ignores).
pub const RAW_CHANNELS: usize = 13;
/// The RAW file of a 256 m region, the only size on Second Life: 851,968 bytes.
pub const REGION_RAW_SIZE: usize = RAW_CHANNELS * 256 * 256;
/// The largest file taken, Aurora's MAX_RAW_FILE: a 2048 m OpenSim var region's fits.
pub const MAX_RAW_FILE: usize = 64 << 20;

/// The last packet's flag of an xfer packet number (LLXferManager::encodePacketNum).
const LAST_PACKET: u32 = crate::mutes::LAST_PACKET;
/// LL_XFER_CHUNK_SIZE, and LL_XFER_LARGE_PAYLOAD when the region asks for big packets.
const CHUNK: usize = 1000;
const LARGE_CHUNK: usize = 7680;
/// How long the region may take to ask for the file (Aurora's START_TIMEOUT; the viewer waits
/// forever).
const START_TIMEOUT: Duration = Duration::from_secs(120);
/// An unconfirmed packet is sent again after this, three times (LLXfer's resend, Aurora's).
const RESEND: Duration = Duration::from_secs(3);
const RETRIES: u8 = 3;
/// ELLPath LL_PATH_NONE: the name is the viewer's own file, not one in a known folder.
const LL_PATH_NONE: u8 = 0;

/// The width of the square region a RAW file of `len` bytes is for, in metres: 13 bytes a point,
/// a multiple of 256 m a side (OpenSim's var regions), no larger than [`MAX_RAW_FILE`]. The
/// viewer sends any file; a file the region cannot read would only fail there.
pub fn raw_width(len: usize) -> Option<usize> {
    if len == 0 || len > MAX_RAW_FILE || !len.is_multiple_of(RAW_CHANNELS) {
        return None;
    }
    let points = len / RAW_CHANNELS;
    let side = points.isqrt();
    (side * side == points && side.is_multiple_of(256)).then_some(side)
}

/// Packet `index` of `data` in chunks of `chunk` bytes: the first carries the file's size before
/// its data (an S32, little-endian), the last is flagged. None past the end (Aurora's `packet`,
/// LLXfer::sendNextPacket).
fn packet(data: &[u8], chunk: usize, index: u32) -> Option<(u32, Vec<u8>)> {
    let start = (index as usize).checked_mul(chunk)?;
    if start >= data.len() {
        return None;
    }
    let end = (start + chunk).min(data.len());
    let mut bytes = Vec::with_capacity(end - start + 4);
    if index == 0 {
        bytes.extend_from_slice(&(data.len() as i32).to_le_bytes());
    }
    bytes.extend_from_slice(&data[start..end]);
    let last = if end == data.len() { LAST_PACKET } else { 0 };
    Some((index | last, bytes))
}

/// What the bot sends next, or how the upload ended.
#[derive(Debug, PartialEq)]
pub(crate) enum Step {
    /// Not this upload's.
    Ignore,
    /// SendXferPacket: this packet number and its data.
    Send(u32, Vec<u8>),
    /// Every packet confirmed.
    Done,
    Failed(String),
}

/// The transfer, once the region asked for the file.
#[derive(Debug)]
struct Outgoing {
    id: u64,
    chunk: usize,
    /// The packet sent and not yet confirmed.
    packet: u32,
    sent: Instant,
    retries: u8,
}

/// An upload under way: asked for with `terrain` "upload filename", then sent by xfer.
#[derive(Debug)]
pub(crate) struct Upload {
    name: String,
    data: Arc<Vec<u8>>,
    asked: Instant,
    out: Option<Outgoing>,
    /// Told how the upload ended.
    done: Option<oneshot::Sender<Result<(), String>>>,
}

impl Upload {
    pub fn new(name: String, data: Vec<u8>, done: oneshot::Sender<Result<(), String>>) -> Upload {
        Upload {
            name,
            data: Arc::new(data),
            asked: Instant::now(),
            out: None,
            done: Some(done),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// The xfer under way, once the region asked for the file.
    pub fn xfer_id(&self) -> Option<u64> {
        self.out.as_ref().map(|o| o.id)
    }

    /// Tell whoever waits how the upload ended.
    pub fn finish(&mut self, result: Result<(), String>) {
        if let Some(done) = self.done.take() {
            let _ = done.send(result);
        }
    }

    fn current(&self) -> Step {
        let Some(out) = &self.out else {
            return Step::Ignore;
        };
        match packet(&self.data, out.chunk, out.packet) {
            Some((n, data)) => Step::Send(n, data),
            None => Step::Done,
        }
    }

    /// RequestXfer: the region asks for a file. Ours when it names this upload's file, outside
    /// any known folder and not a cached asset (LLXferManager::processFileRequest with
    /// validateFileForTransfer, as Aurora): then its first packet.
    pub fn requested(
        &mut self,
        id: u64,
        filename: &str,
        file_path: u8,
        vfile_nil: bool,
        big_packets: bool,
        now: Instant,
    ) -> Step {
        if self.out.is_some() || !vfile_nil || file_path != LL_PATH_NONE || filename != self.name {
            return Step::Ignore;
        }
        self.out = Some(Outgoing {
            id,
            chunk: if big_packets { LARGE_CHUNK } else { CHUNK },
            packet: 0,
            sent: now,
            retries: 0,
        });
        self.current()
    }

    /// ConfirmXferPacket: the next packet, or the end once the last one is confirmed.
    pub fn confirmed(&mut self, id: u64, packet: u32, now: Instant) -> Step {
        let Some(out) = self.out.as_mut().filter(|o| o.id == id) else {
            return Step::Ignore;
        };
        if packet & !LAST_PACKET != out.packet {
            // an older confirmation, sent again
            return Step::Ignore;
        }
        out.packet += 1;
        out.sent = now;
        out.retries = 0;
        self.current()
    }

    /// The timers: the region never asked for the file, or a packet stays unconfirmed (it goes
    /// again, three times).
    pub fn tick(&mut self, now: Instant) -> Step {
        match self.out.as_mut() {
            None if now.duration_since(self.asked) > START_TIMEOUT => {
                Step::Failed("the region never asked for the file".into())
            }
            None => Step::Ignore,
            Some(o) if now.duration_since(o.sent) > RESEND => {
                if o.retries >= RETRIES {
                    return Step::Failed("the region stopped confirming the file's packets".into());
                }
                o.retries += 1;
                o.sent = now;
                self.current()
            }
            Some(_) => Step::Ignore,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_files_are_square_and_13_bytes_a_point() {
        assert_eq!(REGION_RAW_SIZE, 851_968);
        assert_eq!(raw_width(REGION_RAW_SIZE), Some(256));
        assert_eq!(raw_width(RAW_CHANNELS * 512 * 512), Some(512));
        assert_eq!(raw_width(RAW_CHANNELS * 2048 * 2048), Some(2048));
        // too large, not square, not 13 channels, not a whole region
        assert_eq!(raw_width(RAW_CHANNELS * 4096 * 4096), None);
        assert_eq!(raw_width(RAW_CHANNELS * 256 * 512), None);
        assert_eq!(raw_width(12 * 256 * 256), None);
        assert_eq!(raw_width(RAW_CHANNELS * 255 * 255), None);
        assert_eq!(raw_width(0), None);
    }

    /// 2,500 bytes in 1,000-byte packets: three packets, the last flagged, the first with the
    /// size before (Aurora's the_file_goes_in_numbered_packets).
    #[test]
    fn the_file_goes_in_numbered_packets() {
        let data = vec![7u8; 2500];
        let (i0, p0) = packet(&data, CHUNK, 0).expect("first");
        assert_eq!((i0, p0.len()), (0, 1004));
        assert_eq!(&p0[..4], &2500i32.to_le_bytes());
        let (i2, p2) = packet(&data, CHUNK, 2).expect("last");
        assert_eq!((i2, p2.len()), (2 | LAST_PACKET, 500));
        assert!(packet(&data, CHUNK, 3).is_none());
        // a file of one packet: the first is the last
        let (i, p) = packet(&[1, 2, 3], CHUNK, 0).expect("only");
        assert_eq!((i, p.len()), (LAST_PACKET, 7));
    }

    #[test]
    fn an_upload_goes_packet_by_packet() {
        let (tx, mut rx) = oneshot::channel();
        let data: Vec<u8> = (0..2500u32).map(|i| i as u8).collect();
        let mut u = Upload::new("terrain.raw".into(), data.clone(), tx);
        let t = Instant::now();
        // another file, a cached asset, another folder: not ours
        assert_eq!(u.requested(9, "other.raw", 0, true, false, t), Step::Ignore);
        assert_eq!(
            u.requested(9, "terrain.raw", 0, false, false, t),
            Step::Ignore
        );
        assert_eq!(
            u.requested(9, "terrain.raw", 4, true, false, t),
            Step::Ignore
        );
        assert_eq!(u.xfer_id(), None);

        let Step::Send(0, first) = u.requested(9, "terrain.raw", 0, true, false, t) else {
            panic!("the first packet");
        };
        assert_eq!(u.xfer_id(), Some(9));
        let mut sent = first[4..].to_vec();
        // a confirmation of another xfer, or of no packet sent
        assert_eq!(u.confirmed(8, 0, t), Step::Ignore);
        assert_eq!(u.confirmed(9, 5, t), Step::Ignore);
        let Step::Send(1, second) = u.confirmed(9, 0, t) else {
            panic!("the second packet");
        };
        sent.extend_from_slice(&second);
        // the same confirmation again changes nothing
        assert_eq!(u.confirmed(9, 0, t), Step::Ignore);
        let Step::Send(n, last) = u.confirmed(9, 1, t) else {
            panic!("the last packet");
        };
        assert_eq!(n, 2 | LAST_PACKET);
        sent.extend_from_slice(&last);
        assert_eq!(sent, data);
        assert_eq!(u.confirmed(9, 2 | LAST_PACKET, t), Step::Done);
        u.finish(Ok(()));
        assert_eq!(rx.try_recv(), Ok(Ok(())));
    }

    #[test]
    fn an_upload_resends_then_gives_up() {
        let (tx, _rx) = oneshot::channel();
        let mut u = Upload::new("terrain.raw".into(), vec![0; 2500], tx);
        let t = Instant::now();
        assert_eq!(u.tick(t + Duration::from_secs(60)), Step::Ignore);
        assert!(matches!(u.tick(t + START_TIMEOUT * 2), Step::Failed(_)));

        let (tx, _rx) = oneshot::channel();
        let mut u = Upload::new("terrain.raw".into(), vec![0; 2500], tx);
        assert!(matches!(
            u.requested(1, "terrain.raw", 0, true, true, t),
            Step::Send(_, p) if p.len() == 2504
        ));
        let mut at = t;
        for _ in 0..RETRIES {
            at += RESEND * 2;
            assert!(matches!(u.tick(at), Step::Send(LAST_PACKET, _)));
        }
        assert!(matches!(u.tick(at + RESEND * 2), Step::Failed(_)));
    }
}
