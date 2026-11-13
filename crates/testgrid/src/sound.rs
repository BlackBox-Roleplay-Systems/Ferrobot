//! Sound files for the tests to serve: an Ogg Vorbis file's pages and headers (RFC 3533, the
//! Vorbis I identification, comment and setup headers), with stand-ins for its audio packets.
//! Written apart from the bot's checks on purpose, its CRC by a table, so that a mistake in one
//! is not hidden by the same one in the other.

/// The Ogg CRC's table: polynomial 0x04c11db7, not reflected.
fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    for (i, entry) in table.iter_mut().enumerate() {
        let mut r = (i as u32) << 24;
        for _ in 0..8 {
            r = if r & 0x8000_0000 != 0 {
                (r << 1) ^ 0x04c1_1db7
            } else {
                r << 1
            };
        }
        *entry = r;
    }
    table
}

fn page(table: &[u32; 256], flags: u8, granule: u64, sequence: u32, packet: &[u8]) -> Vec<u8> {
    let mut lacing: Vec<u8> = vec![255; packet.len() / 255];
    lacing.push((packet.len() % 255) as u8);
    let mut p = b"OggS\0".to_vec();
    p.push(flags);
    p.extend_from_slice(&granule.to_le_bytes());
    p.extend_from_slice(&0x7e57_0001u32.to_le_bytes());
    p.extend_from_slice(&sequence.to_le_bytes());
    p.extend_from_slice(&[0; 4]);
    p.push(lacing.len() as u8);
    p.extend_from_slice(&lacing);
    p.extend_from_slice(packet);
    let crc = p.iter().fold(0u32, |crc, &b| {
        (crc << 8) ^ table[((crc >> 24) as u8 ^ b) as usize]
    });
    p[22..26].copy_from_slice(&crc.to_le_bytes());
    p
}

/// An Ogg Vorbis file of `seconds` (whole), `channels` and `rate`, in one page per second.
pub fn ogg_vorbis(channels: u8, rate: u32, seconds: u32) -> Vec<u8> {
    let table = crc_table();
    let mut id = b"\x01vorbis".to_vec();
    id.extend_from_slice(&0u32.to_le_bytes());
    id.push(channels);
    id.extend_from_slice(&rate.to_le_bytes());
    id.extend_from_slice(&[0, 0, 0, 0, 0x80, 0x38, 0x01, 0, 0, 0, 0, 0]);
    id.extend_from_slice(&[0xb8, 0x01]);
    let mut out = page(&table, 0x02, 0, 0, &id);
    let mut comment = b"\x03vorbis".to_vec();
    comment.extend_from_slice(&8u32.to_le_bytes());
    comment.extend_from_slice(b"testgrid");
    comment.extend_from_slice(&0u32.to_le_bytes());
    comment.push(1);
    out.extend(page(&table, 0, 0, 1, &comment));
    let mut setup = b"\x05vorbis".to_vec();
    setup.extend((0..300u32).map(|i| (i * 7) as u8));
    out.extend(page(&table, 0, 0, 2, &setup));
    for s in 1..=seconds {
        let audio: Vec<u8> = (0..400u32).map(|i| (i ^ s) as u8).collect();
        let flags = if s == seconds { 0x04 } else { 0 };
        let granule = u64::from(s) * u64::from(rate);
        out.extend(page(&table, flags, granule, 2 + s, &audio));
    }
    out
}
