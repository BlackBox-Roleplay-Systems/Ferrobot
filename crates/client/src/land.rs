//! The region's land as a viewer sees it: the ground's height (LayerData patches) and the parcel
//! overlay (ParcelOverlay), and the land tools' values (ModifyLand).
//!
//! The patch decoder is a port of aurora-net/src/terrain.rs (Aurora Viewer, see NOTICE.md),
//! itself a port of llmessage/patch_code.cpp, patch_idct.cpp and llcommon/llbitpack.h (originally
//! LGPL 2.1). The overlay follows LLViewerParcelOverlay (newview/llviewerparceloverlay.cpp).

use std::collections::HashMap;

/// LayerData's layer types: land in 16 m patches, land in a var region (32-bit patch ids).
pub const LAND_LAYER: u8 = b'L';
pub const LAND_LAYER_LARGE: u8 = b'M';
const END_OF_PATCHES: u8 = 97;
const OO_SQRT2: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// ModifyLand's actions (LLToolBrushLand's E_LAND_LEVEL…, the viewer's land tools).
pub mod action {
    pub const FLATTEN: u8 = 0;
    pub const RAISE: u8 = 1;
    pub const LOWER: u8 = 2;
    pub const SMOOTH: u8 = 3;
    pub const ROUGHEN: u8 = 4;
    pub const REVERT: u8 = 5;
}

/// The legacy brush index of ModifyBlock (LLToolBrushLand::getBrushIndex): sizes up to 2 m are
/// 0, up to 4 m 1, larger 2. The real size goes in ModifyBlockExtended.
pub fn brush_index(size: f32) -> u8 {
    if size > 4.0 {
        2
    } else if size > 2.0 {
        1
    } else {
        0
    }
}

/// One decoded patch: `size * size` heights, row by row from the south.
#[derive(Debug, Clone, PartialEq)]
pub struct Patch {
    pub x: u32,
    pub y: u32,
    pub size: u32,
    pub heights: Vec<f32>,
}

/// The ground of the bot's region, as the patches the region has sent so far.
#[derive(Debug, Clone, Default)]
pub struct Terrain {
    patches: HashMap<(u32, u32), Patch>,
    /// Patches received since the arrival (each LayerData counts each of its patches).
    pub updates: u64,
}

impl Terrain {
    pub fn add(&mut self, patches: Vec<Patch>) {
        for p in patches {
            self.updates += 1;
            self.patches.insert((p.x, p.y), p);
        }
    }

    /// The ground's height at a point of the region (the nearest sample, as the patch has it),
    /// `None` while its patch has not come.
    pub fn height(&self, x: f32, y: f32) -> Option<f32> {
        if !(x.is_finite() && y.is_finite()) || x < 0.0 || y < 0.0 {
            return None;
        }
        let (xi, yi) = (x as u32, y as u32);
        // every patch of a layer has the same size; 16 m on Second Life and OpenSim
        let size = self.patches.values().next()?.size;
        let p = self.patches.get(&(xi / size, yi / size))?;
        let i = ((yi % size) * size + xi % size) as usize;
        p.heights.get(i).copied()
    }

    /// How many patches are known.
    pub fn len(&self) -> usize {
        self.patches.len()
    }

    pub fn is_empty(&self) -> bool {
        self.patches.is_empty()
    }
}

/// What a cell of the overlay is (PARCEL_* of llparcelflags.h): its owner, as seen by the bot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellOwner {
    Public,
    Other,
    Group,
    Mine,
    ForSale,
    Auction,
}

/// The parcel overlay: one byte for each 4 m cell of the region, as ParcelOverlay sends them
/// (1024 cells a message, by sequence): who owns the cell and the property lines on its west and
/// south sides.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overlay {
    parts: Vec<Option<Vec<u8>>>,
    /// Messages received since the arrival: the region sends the whole overlay again after a
    /// parcel changes.
    pub updates: u64,
}

/// PARCEL_OWNER_MASK, PARCEL_WEST_LINE, PARCEL_SOUTH_LINE.
const OWNER_MASK: u8 = 0x07;
const WEST_LINE: u8 = 0x40;
const SOUTH_LINE: u8 = 0x80;
/// Cells of a ParcelOverlay message (PARCEL_OVERLAY_CHUNKS of a 256 m region's 4096).
pub const CELLS_PER_PART: usize = 1024;

impl Overlay {
    pub fn add(&mut self, sequence: i32, data: &[u8]) {
        let Ok(i) = usize::try_from(sequence) else {
            return;
        };
        // a 2048 m var region has 256 parts; nothing larger exists
        if i >= 256 {
            return;
        }
        if self.parts.len() <= i {
            self.parts.resize(i + 1, None);
        }
        self.parts[i] = Some(data.to_vec());
        self.updates += 1;
    }

    /// The cells known, in order, once every part up to the last received has come.
    fn cells(&self) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        for p in &self.parts {
            out.extend_from_slice(p.as_ref()?);
        }
        Some(out)
    }

    /// Cells on a side, when the parts received make a square.
    pub fn width(&self) -> Option<usize> {
        let n = self.cells()?.len();
        let w = (n as f64).sqrt() as usize;
        (w > 0 && w * w == n).then_some(w)
    }

    fn cell(&self, x: usize, y: usize) -> Option<u8> {
        let w = self.width()?;
        (x < w && y < w).then(|| self.cells().map(|c| c[y * w + x]))?
    }

    pub fn owner(&self, x: usize, y: usize) -> Option<CellOwner> {
        Some(match self.cell(x, y)? & OWNER_MASK {
            0 => CellOwner::Public,
            1 => CellOwner::Other,
            2 => CellOwner::Group,
            3 => CellOwner::Mine,
            4 => CellOwner::ForSale,
            _ => CellOwner::Auction,
        })
    }

    /// A property line on the cell's west side.
    pub fn west_line(&self, x: usize, y: usize) -> Option<bool> {
        Some(self.cell(x, y)? & WEST_LINE != 0)
    }

    /// A property line on the cell's south side.
    pub fn south_line(&self, x: usize, y: usize) -> Option<bool> {
        Some(self.cell(x, y)? & SOUTH_LINE != 0)
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    load: u8,
    load_size: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            load: 0,
            load_size: 0,
        }
    }

    /// LLBitPack::bitUnpack into a little-endian integer: bits read most significant first, 8 at a
    /// time, each 8 filling the next byte.
    fn unpack(&mut self, mut count: u32) -> Option<u32> {
        let mut result: u32 = 0;
        let mut byte_index = 0;
        while count > 0 {
            let dsize = count.min(8);
            count -= dsize;
            let mut byte: u8 = 0;
            for _ in 0..dsize {
                if self.load_size == 0 {
                    self.load = *self.data.get(self.pos)?;
                    self.pos += 1;
                    self.load_size = 8;
                }
                byte = (byte << 1) | (self.load >> 7);
                self.load_size -= 1;
                self.load <<= 1;
            }
            if byte_index < 4 {
                result |= u32::from(byte) << (8 * byte_index);
            }
            byte_index += 1;
        }
        Some(result)
    }
}

struct Tables {
    size: usize,
    dequant: Vec<f32>,
    icos: Vec<f32>,
    decopy: Vec<usize>,
}

impl Tables {
    fn new(size: usize) -> Tables {
        let mut dequant = vec![0.0; size * size];
        for j in 0..size {
            for i in 0..size {
                dequant[j * size + i] = 1.0 + 2.0 * (i + j) as f32;
            }
        }
        let mut icos = vec![0.0; size * size];
        let oosob = std::f32::consts::PI * 0.5 / size as f32;
        for u in 0..size {
            for n in 0..size {
                icos[u * size + n] = ((2.0 * n as f32 + 1.0) * u as f32 * oosob).cos();
            }
        }
        // the zig-zag order of the coefficients
        let mut decopy = vec![0usize; size * size];
        let (mut i, mut j) = (0usize, 0usize);
        let mut count = 0usize;
        let mut diag = false;
        let mut right = true;
        while i < size && j < size {
            decopy[j * size + i] = count;
            count += 1;
            if !diag {
                if right {
                    if i < size - 1 {
                        i += 1;
                    } else {
                        j += 1;
                    }
                    right = false;
                } else {
                    if j < size - 1 {
                        j += 1;
                    } else {
                        i += 1;
                    }
                    right = true;
                }
                diag = true;
            } else if right {
                i += 1;
                j = j.wrapping_sub(1);
                if i == size - 1 || j == 0 {
                    diag = false;
                }
            } else {
                i = i.wrapping_sub(1);
                j += 1;
                if i == 0 || j == size - 1 {
                    diag = false;
                }
            }
        }
        Tables {
            size,
            dequant,
            icos,
            decopy,
        }
    }

    fn idct(&self, block: &mut [f32]) {
        let size = self.size;
        let mut temp = vec![0.0f32; size * size];
        for column in 0..size {
            for n in 0..size {
                let mut total = OO_SQRT2 * block[column];
                for u in 1..size {
                    total += block[u * size + column] * self.icos[u * size + n];
                }
                temp[size * n + column] = total;
            }
        }
        let oosob = 2.0 / size as f32;
        for line in 0..size {
            let ls = line * size;
            for n in 0..size {
                let mut total = OO_SQRT2 * temp[ls];
                for u in 1..size {
                    total += temp[ls + u] * self.icos[u * size + n];
                }
                block[ls + n] = total * oosob;
            }
        }
    }
}

/// The patches of a land LayerData (`L`, or `M` in a var region); other layers give none.
pub fn decode_land_layer(layer_type: u8, data: &[u8]) -> Vec<Patch> {
    let large = match layer_type {
        LAND_LAYER => false,
        LAND_LAYER_LARGE => true,
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut br = BitReader::new(data);
    // the group header: stride, patch size, layer type
    let (Some(_), Some(patch_size), Some(_)) = (br.unpack(16), br.unpack(8), br.unpack(8)) else {
        return out;
    };
    let size = patch_size as usize;
    if size != 16 && size != 32 {
        return out;
    }
    let tables = Tables::new(size);
    let mut coeffs = vec![0i32; size * size];
    while let Some(quant_wbits) = br.unpack(8) {
        let quant_wbits = quant_wbits as u8;
        if quant_wbits == END_OF_PATCHES {
            break;
        }
        let (Some(dc_bits), Some(range), Some(ids)) = (
            br.unpack(32),
            br.unpack(16),
            br.unpack(if large { 32 } else { 10 }),
        ) else {
            break;
        };
        let dc_offset = f32::from_bits(dc_bits);
        let (px, py) = if large {
            (ids >> 16, ids & 0xFFFF)
        } else {
            (ids >> 5, ids & 0x1F)
        };
        let wbits = u32::from(quant_wbits & 0x0f) + 2;
        // decode_patch: a 0 bit is a zero, 10 the end of the block, 11 a signed value
        let mut ok = true;
        let mut i = 0;
        while i < size * size {
            let Some(b) = br.unpack(1) else {
                ok = false;
                break;
            };
            if b == 0 {
                coeffs[i] = 0;
                i += 1;
                continue;
            }
            match br.unpack(1) {
                None => {
                    ok = false;
                    break;
                }
                Some(0) => {
                    coeffs[i..].fill(0);
                    break;
                }
                Some(_) => {
                    let (Some(neg), Some(v)) = (br.unpack(1), br.unpack(wbits)) else {
                        ok = false;
                        break;
                    };
                    coeffs[i] = if neg != 0 { -(v as i32) } else { v as i32 };
                    i += 1;
                }
            }
        }
        if !ok {
            break;
        }
        // decompress_patch
        let prequant = i32::from(quant_wbits >> 4) + 2;
        let quantize = 1i32 << prequant;
        let mult = range as f32 / quantize as f32;
        let addval = mult * (1i32 << (prequant - 1)) as f32 + dc_offset;
        let mut block = vec![0.0f32; size * size];
        for (k, b) in block.iter_mut().enumerate() {
            *b = coeffs[tables.decopy[k]] as f32 * tables.dequant[k];
        }
        tables.idct(&mut block);
        let heights: Vec<f32> = block.iter().map(|v| v * mult + addval).collect();
        if heights.iter().all(|h| h.is_finite()) {
            out.push(Patch {
                x: px,
                y: py,
                size: size as u32,
                heights,
            });
        }
    }
    out
}

/// A land LayerData of flat patches, each at one height: what a region sends, reduced to the
/// mean of each patch (a DC coefficient alone). For tests and the test grid.
pub fn encode_flat_layer(patches: &[(u32, u32, f32)]) -> Vec<u8> {
    let mut bits = BitWriter::default();
    bits.push(264, 16);
    bits.push(16, 8);
    bits.push(u32::from(LAND_LAYER), 8);
    for &(x, y, h) in patches {
        bits.push(0x11, 8);
        bits.push(h.to_bits(), 32);
        bits.push(0, 16);
        bits.push(((x & 0x1f) << 5) | (y & 0x1f), 10);
        // 10: the end of the block, every coefficient zero
        bits.push(1, 1);
        bits.push(0, 1);
    }
    bits.push(u32::from(END_OF_PATCHES), 8);
    bits.bytes()
}

#[derive(Default)]
struct BitWriter {
    bits: Vec<u8>,
}

impl BitWriter {
    /// The inverse of [`BitReader::unpack`].
    fn push(&mut self, v: u32, n: u32) {
        let mut rem = n;
        let mut idx = 0;
        while rem > 0 {
            let d = rem.min(8);
            rem -= d;
            let byte = (v >> (8 * idx)) & 0xFF;
            for b in (0..d).rev() {
                self.bits.push(((byte >> b) & 1) as u8);
            }
            idx += 1;
        }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.bits.len().div_ceil(8)];
        for (i, b) in self.bits.iter().enumerate() {
            out[i / 8] |= b << (7 - (i % 8));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_are_read_as_the_viewer_reads_them() {
        let d = [0xAA, 0xC0];
        let mut br = BitReader::new(&d);
        assert_eq!(br.unpack(1), Some(1));
        assert_eq!(br.unpack(1), Some(0));
        let v = br.unpack(10).expect("bits");
        assert_eq!(v & 0xFF, 0b1010_1011);
        assert_eq!(v >> 8, 0);
    }

    #[test]
    fn the_zig_zag_takes_every_coefficient_once() {
        let t = Tables::new(16);
        let mut seen = [false; 256];
        for &v in &t.decopy {
            seen[v] = true;
        }
        assert!(seen.iter().all(|s| *s));
    }

    #[test]
    fn flat_patches_decode_to_their_heights() {
        let data = encode_flat_layer(&[(3, 4, 20.0), (15, 15, 31.5)]);
        let p = decode_land_layer(LAND_LAYER, &data);
        assert_eq!(p.len(), 2);
        assert_eq!((p[0].x, p[0].y, p[0].size), (3, 4, 16));
        assert!(p[0].heights.iter().all(|h| (h - 20.0).abs() < 1e-3));
        let mut t = Terrain::default();
        t.add(p);
        assert_eq!((t.len(), t.updates), (2, 2));
        assert!((t.height(3.0 * 16.0 + 5.5, 4.0 * 16.0).expect("height") - 20.0).abs() < 1e-3);
        assert!((t.height(255.9, 255.9).expect("height") - 31.5).abs() < 1e-3);
        assert_eq!(t.height(0.0, 0.0), None);
        assert_eq!(t.height(-1.0, 3.0), None);
        // a cut stream, or a wind layer, gives what it can and no more
        assert!(decode_land_layer(LAND_LAYER, &data[..6]).is_empty());
        assert!(decode_land_layer(b'7', &data).is_empty());
    }

    #[test]
    fn the_overlay_is_read_once_whole() {
        let mut o = Overlay::default();
        let mut cells = vec![3u8; CELLS_PER_PART];
        // the cell (2, 1): someone else's, a line on its west side
        cells[64 + 2] = 1 | WEST_LINE;
        o.add(0, &cells);
        o.add(2, &vec![0u8; CELLS_PER_PART]);
        // part 1 missing: not a whole map yet
        assert_eq!(o.width(), None);
        o.add(1, &vec![0u8; CELLS_PER_PART]);
        o.add(3, &vec![SOUTH_LINE | 4; CELLS_PER_PART]);
        assert_eq!(o.width(), Some(64));
        assert_eq!(o.owner(0, 0), Some(CellOwner::Mine));
        assert_eq!(o.owner(2, 1), Some(CellOwner::Other));
        assert_eq!(o.west_line(2, 1), Some(true));
        assert_eq!(o.west_line(3, 1), Some(false));
        assert_eq!(o.owner(5, 63), Some(CellOwner::ForSale));
        assert_eq!(o.south_line(5, 63), Some(true));
        assert_eq!(o.owner(64, 0), None);
        assert_eq!(o.updates, 4);
    }

    #[test]
    fn brush_sizes_have_their_old_index() {
        assert_eq!([1.0, 2.0, 3.0, 4.0, 8.0].map(brush_index), [0, 0, 1, 1, 2]);
    }
}
