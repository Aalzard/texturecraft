//! Reader for the SubCraft raster blob (`rasters/NNNNN.tiles`) inside a `.scsmart` container.
//!
//! Layout (little endian), as documented by the public SubCraft file format: the magic `SCTILE`,
//! a `u16` blob version (1), `u32` width, `u32` height, a `u8` component count (1..=4), four
//! `f32` fill values (what an unallocated tile holds), a `u32` allocated-tile count, then per
//! allocated tile a `u32` tile index (row-major, 64×64 texel tiles) and `64·64·comps` IEEE half
//! floats, texel-major with the components interleaved.
//!
//! Every number in the blob is hostile: sizes are bounded before anything is allocated and a
//! short or inconsistent blob is an error, never a panic.

use std::collections::BTreeSet;

use super::Budget;

const MAGIC: &[u8; 6] = b"SCTILE";
const VERSION: u16 = 1;
/// Tile edge in texels (the same as TextureCraft's own tiles).
pub(super) const TILE: u32 = 64;
pub(super) const TILE_AREA: usize = (TILE * TILE) as usize;
/// Largest raster edge SubCraft writes (its biggest texture set resolution).
const MAX_EDGE: u32 = 4096;

/// A decoded raster: the fill value and the allocated tiles, still as half-float bits.
#[derive(Clone, Debug)]
pub(super) struct Raster {
    pub width: u32,
    pub height: u32,
    pub comps: usize,
    pub fill: [f32; 4],
    /// `(tile index, TILE_AREA × comps half-float bit patterns)`.
    pub tiles: Vec<(usize, Vec<u16>)>,
}

impl Raster {
    pub fn tiles_x(&self) -> u32 {
        self.width.div_ceil(TILE)
    }
}

/// Half-float bits to `f32`; NaN and infinities read as 0 so they never reach the compositor.
pub(super) fn half_to_f32(bits: u16) -> f32 {
    let v = half::f16::from_bits(bits).to_f32();
    if v.is_finite() { v } else { 0.0 }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).ok_or("truncated raster")?;
        let s = self.bytes.get(self.at..end).ok_or("truncated raster")?;
        self.at = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?.first().copied().unwrap_or(0))
    }
    fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b.first().copied().unwrap_or(0), b.get(1).copied().unwrap_or(0)]))
    }
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([
            b.first().copied().unwrap_or(0),
            b.get(1).copied().unwrap_or(0),
            b.get(2).copied().unwrap_or(0),
            b.get(3).copied().unwrap_or(0),
        ]))
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(self.u32()?))
    }
}

/// Decode a raster blob that must be `resolution` × `resolution` with `comps` components.
/// The tiles it allocates are charged to `budget`.
pub(super) fn decode(bytes: &[u8], resolution: u32, comps: usize, budget: &mut Budget) -> Result<Raster, String> {
    let mut r = Reader { bytes, at: 0 };
    if r.take(MAGIC.len())? != MAGIC {
        return Err("not a SubCraft raster blob".into());
    }
    let version = r.u16()?;
    if version != VERSION {
        return Err(format!("unsupported raster blob version {version}"));
    }
    let (width, height) = (r.u32()?, r.u32()?);
    let found = usize::from(r.u8()?);
    if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
        return Err(format!("raster size {width}x{height} is out of range"));
    }
    if (width, height) != (resolution, resolution) {
        return Err(format!("raster is {width}x{height}, the smart material is {resolution}x{resolution}"));
    }
    if found != comps {
        return Err(format!("raster has {found} components, {comps} expected"));
    }
    let mut fill = [0.0f32; 4];
    for slot in &mut fill {
        let v = r.f32()?;
        *slot = if v.is_finite() { v } else { 0.0 };
    }
    let count = r.u32()? as usize;
    let tiles_x = width.div_ceil(TILE) as usize;
    let tile_count = tiles_x * height.div_ceil(TILE) as usize;
    if count > tile_count {
        return Err(format!("{count} tiles in a raster of {tile_count}"));
    }
    let tile_bytes = TILE_AREA * comps * 2;
    // The declared count must fit in what is actually there before anything is reserved.
    let needed = count.checked_mul(4 + tile_bytes).ok_or("raster size overflow")?;
    if bytes.len().saturating_sub(r.at) < needed {
        return Err("truncated raster".into());
    }
    budget.charge(count as u64 * tile_bytes as u64)?;
    let mut seen = BTreeSet::new();
    let mut tiles = Vec::with_capacity(count);
    for _ in 0..count {
        let index = r.u32()? as usize;
        if index >= tile_count || !seen.insert(index) {
            return Err(format!("invalid tile index {index}"));
        }
        let raw = r.take(tile_bytes)?;
        let data = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c.first().copied().unwrap_or(0), c.get(1).copied().unwrap_or(0)])).collect();
        tiles.push((index, data));
    }
    Ok(Raster { width, height, comps, fill, tiles })
}
