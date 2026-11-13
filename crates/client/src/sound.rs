//! Sounds the bot uploads: an Ogg Vorbis file, read as far as its pages and its Vorbis headers,
//! never decoded (the container has no audio library, so the bot cannot encode a WAV file as the
//! viewer's LLVorbisEncode does). What the bot takes is what the viewer makes and the grid
//! expects (indra/llaudio/llvorbisencode.cpp, originally LGPL 2.1): one logical Vorbis stream,
//! mono, 44100 Hz, no longer than LLVORBIS_CLIP_MAX_TIME_SEC. The Ogg pages follow RFC 3533, the
//! identification header the Vorbis I specification.

/// LLVORBIS_CLIP_SAMPLE_RATE
pub const SAMPLE_RATE: u32 = 44_100;
/// LLVORBIS_CLIP_MAX_TIME_SEC (llvorbisencode.h)
pub const MAX_SECONDS: u32 = 30;

/// What the headers say of a sound.
#[derive(Debug, Clone, PartialEq)]
pub struct OggVorbis {
    pub channels: u8,
    pub sample_rate: u32,
    /// The last page's granule position: the samples in the stream.
    pub samples: u64,
}

impl OggVorbis {
    pub fn seconds(&self) -> f64 {
        self.samples as f64 / f64::from(self.sample_rate.max(1))
    }
}

/// A page's header flags (RFC 3533 section 6).
const BEGIN: u8 = 0x02;
const END: u8 = 0x04;
/// The fixed part of a page's header.
const HEADER: usize = 27;

/// The Ogg checksum: CRC-32, polynomial 0x04c11db7, not reflected, from 0, with the page's own
/// checksum field read as zeros (RFC 3533 section 6).
fn ogg_crc(page: &[u8]) -> u32 {
    let mut crc: u32 = 0;
    for (i, &b) in page.iter().enumerate() {
        let b = if (22..26).contains(&i) { 0 } else { b };
        crc ^= u32::from(b) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04c1_1db7
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The Vorbis identification header (the first packet): channels and sample rate.
fn identification(body: &[u8]) -> Result<(u8, u32), String> {
    if body.len() < 30 || body[0] != 1 || &body[1..7] != b"vorbis" {
        return Err("its first packet is not a Vorbis identification header".into());
    }
    if u32_at(body, 7) != 0 {
        return Err("an unknown Vorbis version".into());
    }
    if body[29] & 1 == 0 {
        return Err("a broken Vorbis identification header".into());
    }
    Ok((body[11], u32_at(body, 12)))
}

/// Checks a sound before it is uploaded: why not, in words for a command's reply.
pub fn check(data: &[u8]) -> Result<OggVorbis, String> {
    if data.get(..4) == Some(b"RIFF") && data.get(8..12) == Some(b"WAVE") {
        return Err(format!(
            "a WAV file, which the bot cannot encode: send it as Ogg Vorbis (mono, {SAMPLE_RATE} \
             Hz, {MAX_SECONDS} s at most)"
        ));
    }
    if data.get(..4) != Some(b"OggS") {
        return Err("not an Ogg file".into());
    }
    let mut pos = 0;
    let mut pages: u32 = 0;
    let mut serial = 0;
    let mut ended = false;
    let mut samples = 0;
    let mut format = None;
    while pos < data.len() {
        if ended {
            return Err("data after the end of its stream".into());
        }
        let head = data
            .get(pos..pos + HEADER)
            .ok_or("a page cut short (a file cut short?)")?;
        if &head[..4] != b"OggS" || head[4] != 0 {
            return Err(format!("no Ogg page at byte {pos}"));
        }
        let flags = head[5];
        let granule = u64::from_le_bytes([
            head[6], head[7], head[8], head[9], head[10], head[11], head[12], head[13],
        ]);
        let segments = usize::from(head[26]);
        let table = data
            .get(pos + HEADER..pos + HEADER + segments)
            .ok_or("a page cut short (a file cut short?)")?;
        let body_len: usize = table.iter().map(|&s| usize::from(s)).sum();
        let body_at = pos + HEADER + segments;
        let page = data
            .get(pos..body_at + body_len)
            .ok_or("a page cut short (a file cut short?)")?;
        if ogg_crc(page) != u32_at(head, 22) {
            return Err(format!("page {pages} is damaged (its checksum is wrong)"));
        }
        if pages == 0 {
            if flags & BEGIN == 0 {
                return Err("its first page does not begin a stream".into());
            }
            serial = u32_at(head, 14);
            format = Some(identification(&page[HEADER + segments..])?);
        } else if flags & BEGIN != 0 || u32_at(head, 14) != serial {
            return Err("more than one stream in it".into());
        }
        if u32_at(head, 18) != pages {
            return Err(format!("page {pages} is missing"));
        }
        let body = &page[HEADER + segments..];
        if pages == 1 && (body.len() < 7 || body[0] != 3 || &body[1..7] != b"vorbis") {
            return Err("no Vorbis comment header after the first page".into());
        }
        // no packet ends on a page whose granule position is -1
        if granule != u64::MAX {
            samples = granule;
        }
        ended = flags & END != 0;
        pages += 1;
        pos = body_at + body_len;
    }
    if !ended {
        return Err("its stream does not end (a file cut short?)".into());
    }
    let (channels, sample_rate) = format.ok_or("no Vorbis header")?;
    if channels != 1 {
        return Err(format!("{channels} channels, the grid takes mono sounds"));
    }
    if sample_rate != SAMPLE_RATE {
        return Err(format!(
            "{sample_rate} Hz, the grid takes {SAMPLE_RATE} Hz sounds"
        ));
    }
    let sound = OggVorbis {
        channels,
        sample_rate,
        samples,
    };
    if samples == 0 {
        return Err("no sound in it".into());
    }
    if sound.seconds() > f64::from(MAX_SECONDS) {
        return Err(format!(
            "{:.1} s long, the grid takes {MAX_SECONDS} s at most",
            sound.seconds()
        ));
    }
    Ok(sound)
}

/// An Ogg page around these packets' bytes (each packet under 255 bytes), for tests.
#[cfg(test)]
fn page(flags: u8, granule: u64, serial: u32, sequence: u32, packets: &[&[u8]]) -> Vec<u8> {
    let mut p = b"OggS".to_vec();
    p.push(0);
    p.push(flags);
    p.extend_from_slice(&granule.to_le_bytes());
    p.extend_from_slice(&serial.to_le_bytes());
    p.extend_from_slice(&sequence.to_le_bytes());
    p.extend_from_slice(&[0; 4]);
    p.push(packets.len() as u8);
    for packet in packets {
        p.push(packet.len() as u8);
    }
    for packet in packets {
        p.extend_from_slice(packet);
    }
    let crc = ogg_crc(&p);
    p[22..26].copy_from_slice(&crc.to_le_bytes());
    p
}

/// A small Ogg Vorbis file as the checks see one: its headers, then audio pages up to `samples`.
/// Its audio packets are not real Vorbis audio: nothing here decodes them.
#[cfg(test)]
pub fn test_file(channels: u8, sample_rate: u32, samples: u64) -> Vec<u8> {
    let mut id = vec![1];
    id.extend_from_slice(b"vorbis");
    id.extend_from_slice(&0u32.to_le_bytes());
    id.push(channels);
    id.extend_from_slice(&sample_rate.to_le_bytes());
    id.extend_from_slice(&0i32.to_le_bytes());
    id.extend_from_slice(&80_000i32.to_le_bytes());
    id.extend_from_slice(&0i32.to_le_bytes());
    id.push(0xb8);
    id.push(1);
    let mut comment = vec![3];
    comment.extend_from_slice(b"vorbis");
    comment.extend_from_slice(&[0; 8]);
    comment.push(1);
    let mut setup = vec![5];
    setup.extend_from_slice(b"vorbis");
    setup.extend_from_slice(&[0x42; 40]);
    let serial = 0x5eed;
    let mut out = page(BEGIN, 0, serial, 0, &[&id]);
    out.extend(page(0, 0, serial, 1, &[&comment, &setup]));
    let audio = [0x2a; 200];
    out.extend(page(0, samples / 2, serial, 2, &[&audio]));
    out.extend(page(END, samples, serial, 3, &[&audio]));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mono_sound_of_the_viewers_kind() {
        let sound = check(&test_file(1, 44_100, 44_100 * 3)).expect("a sound");
        assert_eq!(sound.channels, 1);
        assert_eq!(sound.seconds(), 3.0);
        assert!(check(&test_file(1, 44_100, 44_100 * 30)).is_ok());
    }

    #[test]
    fn what_the_grid_would_not_take() {
        let why = |data: &[u8]| check(data).expect_err("refused");
        assert!(why(&test_file(2, 44_100, 1000)).contains("2 channels"));
        assert!(why(&test_file(1, 48_000, 1000)).contains("48000 Hz"));
        assert!(why(&test_file(1, 44_100, 44_100 * 31)).contains("31.0 s long"));
        assert!(why(&test_file(1, 44_100, 0)).contains("no sound"));
        let mut wav = b"RIFF\x24\0\0\0WAVEfmt ".to_vec();
        wav.extend_from_slice(&[0; 32]);
        assert!(why(&wav).starts_with("a WAV file"));
        assert_eq!(why(b"ID3\x04 an mp3"), "not an Ogg file");
    }

    #[test]
    fn damaged_and_cut_files() {
        let file = test_file(1, 44_100, 1000);
        let why = |data: &[u8]| check(data).expect_err("refused");
        assert!(why(&file[..file.len() - 10]).contains("cut short"));
        let mut damaged = file.clone();
        let last = damaged.len() - 1;
        damaged[last] ^= 0xff;
        assert!(why(&damaged).contains("checksum"));
        // the last page left out: the stream never ends
        let pages: Vec<usize> = file
            .windows(4)
            .enumerate()
            .filter(|(_, w)| *w == b"OggS")
            .map(|(i, _)| i)
            .collect();
        assert!(why(&file[..pages[3]]).contains("does not end"));
        // a page missing
        let mut gap = file[..pages[2]].to_vec();
        gap.extend_from_slice(&file[pages[3]..]);
        assert!(why(&gap).contains("missing"));
        // another stream after this one
        let mut chained = file.clone();
        chained.extend_from_slice(&file);
        assert!(why(&chained).contains("after the end"));
    }

    #[test]
    fn the_checksum_is_oggs() {
        // CRC-32/CKSUM's check value without its final inversion: libogg's CRC
        assert_eq!(ogg_crc(b"123456789"), 0x89a1_897f);
    }
}
