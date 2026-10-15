//! # texturecraft-bake
//!
//! CPU mesh-map baking. [`bake`] rasterizes one texture set's UV layout with a supersampled
//! [`UvGBuffer`], evaluates every requested [`BakedMap`] per sample in one pass (rayon over rows,
//! BVH ray casts for the high-poly transfer, AO, bent normals and thickness), box-downsamples,
//! and pads the UV islands with [`DilationMap`]. With a high poly, every map but position and
//! world-space direction reads the high-poly surface found along the cage / frontal-rear ray.
//! Sampling is deterministic, so the same request always gives the same pixels.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod maps;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

use rayon::prelude::*;
use texturecraft_core::{BakeSettings, BakedMap};
use texturecraft_geom::glam::Vec2;
use texturecraft_geom::{Bvh, Mesh, UvGBuffer};
use texturecraft_texel::DilationMap;

/// Largest accepted output resolution (per axis).
pub const MAX_RESOLUTION: u32 = 8192;
/// Largest accepted ray count per texel (AO, thickness).
pub const MAX_RAYS: u32 = 4096;
/// Largest accepted output (texels over every UV tile).
pub const MAX_TEXELS: u64 = 1 << 27;
/// Supersampled texels rasterized at once: bounds the G-buffer at ~200 MB for any resolution × AA.
const STRIP_TEXELS: usize = 1 << 24;

/// Shared progress / cancellation flag of a running bake.
#[derive(Debug, Default)]
pub struct Progress {
    /// Basis points, 0..=10_000 (all requested maps done).
    pub done: AtomicU32,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Completed fraction, 0..=1.
    pub fn fraction(&self) -> f32 {
        self.done.load(Relaxed).min(10_000) as f32 / 10_000.0
    }

    /// Ask the bake to stop; it returns [`BakeError::Cancelled`] at the next row.
    pub fn cancel(&self) {
        self.cancel.store(true, Relaxed);
    }
}

/// A high-poly source mesh (all high-definition meshes appended into one).
#[derive(Clone, Copy, Debug)]
pub struct HighPoly<'a> {
    pub mesh: &'a Mesh,
    pub bvh: &'a Bvh,
}

/// One texture set to bake.
#[derive(Clone, Copy, Debug)]
pub struct BakeRequest<'a> {
    pub mesh: &'a Mesh,
    /// BVH of the whole low-poly mesh: every material occludes.
    pub bvh: &'a Bvh,
    pub material: usize,
    pub settings: &'a BakeSettings,
    pub high: Option<HighPoly<'a>>,
    /// Cage: the low poly inflated (same triangles); transfer rays start on it.
    pub cage: Option<&'a Mesh>,
    /// UV tile grid (columns, rows) of the texture set: the maps are `settings.resolution` per
    /// tile, (1, 1) for a classic set. The mesh's UVs are in atlas space (0..1 over the grid).
    pub tiles: (u32, u32),
}

/// Baked maps: row-major `resolution × height`, `BakedMap::components()` floats per texel,
/// rows top → bottom (y = v·H, as [`UvGBuffer`]).
#[derive(Clone, Debug, PartialEq)]
pub struct BakeOutput {
    /// Width in texels.
    pub resolution: u32,
    pub height: u32,
    pub maps: BTreeMap<BakedMap, Vec<f32>>,
}

#[derive(Debug, thiserror::Error)]
pub enum BakeError {
    #[error("bake cancelled")]
    Cancelled,
    #[error("invalid bake request: {0}")]
    Invalid(String),
}

fn validate(req: &BakeRequest) -> Result<(), BakeError> {
    let s = req.settings;
    let invalid = |m: String| Err(BakeError::Invalid(m));
    if s.resolution == 0 || s.resolution > MAX_RESOLUTION {
        return invalid(format!("resolution {} is not in 1..={MAX_RESOLUTION}", s.resolution));
    }
    let (cols, rows) = req.tiles;
    if cols == 0 || rows == 0 || (s.resolution as u64 * cols as u64) * (s.resolution as u64 * rows as u64) > MAX_TEXELS {
        return invalid(format!("{cols} × {rows} UV tiles at {} exceed {MAX_TEXELS} texels", s.resolution));
    }
    if ![1, 2, 4, 8].contains(&s.antialiasing) {
        return invalid(format!("antialiasing {} is not 1, 2, 4 or 8", s.antialiasing));
    }
    if s.rays == 0 || s.rays > MAX_RAYS {
        return invalid(format!("ray count {} is not in 1..={MAX_RAYS}", s.rays));
    }
    if !(s.max_distance.is_finite() && s.max_distance > 0.0) {
        return invalid(format!("max distance {} must be positive", s.max_distance));
    }
    if !s.curvature_strength.is_finite() {
        return invalid("curvature strength is not finite".into());
    }
    if !(s.direction.iter().all(|c| c.is_finite()) && s.direction.iter().any(|c| *c != 0.0)) {
        return invalid(format!("direction {:?} must be finite and non-zero", s.direction));
    }
    if !(s.max_frontal.is_finite() && s.max_frontal >= 0.0 && s.max_rear.is_finite() && s.max_rear >= 0.0) {
        return invalid(format!("frontal/rear distances {}/{} must be finite and ≥ 0", s.max_frontal, s.max_rear));
    }
    req.mesh.validate().map_err(|e| BakeError::Invalid(e.to_string()))?;
    if req.material >= req.mesh.materials.len().max(1) {
        return invalid(format!("material {} out of range ({} materials)", req.material, req.mesh.materials.len()));
    }
    if let Some(h) = &req.high {
        h.mesh.validate().map_err(|e| BakeError::Invalid(format!("high poly: {e}")))?;
    }
    if let Some(c) = req.cage {
        c.validate().map_err(|e| BakeError::Invalid(format!("cage: {e}")))?;
        if c.indices.len() != req.mesh.indices.len() {
            return invalid(format!("the cage has {} triangles, the low poly {}", c.triangle_count(), req.mesh.triangle_count()));
        }
    }
    Ok(())
}

/// `vec![value; n]` that reports allocation failure instead of aborting.
fn alloc<T: Clone>(n: usize, value: T) -> Result<Vec<T>, BakeError> {
    let mut v = Vec::new();
    v.try_reserve_exact(n).map_err(|_| BakeError::Invalid(format!("out of memory allocating {n} texels")))?;
    v.resize(n, value);
    Ok(v)
}

/// Bake the maps in `req.settings.maps` for one material of the low-poly mesh.
pub fn bake(req: &BakeRequest, progress: &Progress) -> Result<BakeOutput, BakeError> {
    bake_in_strips(req, progress, STRIP_TEXELS)
}

fn bake_in_strips(req: &BakeRequest, progress: &Progress, strip_texels: usize) -> Result<BakeOutput, BakeError> {
    validate(req)?;
    let s = req.settings;
    let (tile_res, aa) = (s.resolution as usize, s.antialiasing as usize);
    let (w, h) = (tile_res * req.tiles.0 as usize, tile_res * req.tiles.1 as usize);
    let ctx = maps::Ctx::new(req)?;
    progress.done.store(0, Relaxed);

    // Curvature from the normal map needs the normal map: baked alongside when not requested.
    let from_normal = s.curvature_from_normal && s.maps.contains(&BakedMap::Curvature);
    let mut maps: Vec<BakedMap> = s.maps.iter().copied().collect();
    if from_normal && !maps.contains(&BakedMap::Normal) {
        maps.push(BakedMap::Normal);
    }
    let mut bufs = maps.iter().map(|m| alloc(w * h * m.components(), 0.0f32)).collect::<Result<Vec<_>, _>>()?;
    let mut covered = alloc(w * h, false)?;
    let total = h.max(1) as u64;
    let rows_done = AtomicU64::new(0);
    let strip_err = || BakeError::Invalid("strip out of range".into());

    // Rasterize in horizontal strips of the supersampled image: a strip's G-buffer is built from
    // UVs shifted so that y = v*H - strip_top, which keeps memory bounded at 8K x 8 AA.
    let (sw, full_h) = (w * aa, h * aa);
    let strip_rows = (strip_texels / (sw * aa)).max(1).min(h);
    let mut strip_mesh = Mesh { indices: req.mesh.indices.clone(), submeshes: req.mesh.submeshes.clone(), ..Mesh::default() };
    for y0 in (0..h).step_by(strip_rows) {
        let rows = strip_rows.min(h - y0);
        let (sy0, sh) = (y0 * aa, rows * aa);
        strip_mesh.uvs = req.mesh.uvs.iter().map(|uv| Vec2::new(uv.x, (uv.y * full_h as f32 - sy0 as f32) / sh as f32)).collect();
        let g = UvGBuffer::new(&strip_mesh, req.material, sw as u32, sh as u32);
        let subsamples = |r: usize, x: usize| (0..aa * aa).map(move |k| (r * aa + k / aa) * sw + x * aa + k % aa);

        // Per output row: its coverage and its slice of every map.
        let cov = covered.get_mut(y0 * w..(y0 + rows) * w).ok_or_else(strip_err)?;
        let mut out_rows: Vec<(&mut [bool], Vec<&mut [f32]>)> = cov.chunks_mut(w).map(|c| (c, Vec::with_capacity(maps.len()))).collect();
        for (m, data) in maps.iter().zip(bufs.iter_mut()) {
            let comps = m.components();
            let strip = data.get_mut(y0 * w * comps..(y0 + rows) * w * comps).ok_or_else(strip_err)?;
            for ((_, row), chunk) in out_rows.iter_mut().zip(strip.chunks_mut(w * comps)) {
                row.push(chunk);
            }
        }
        out_rows.par_iter_mut().enumerate().try_for_each(|(r, (cov, row))| {
            if progress.cancel.load(Relaxed) {
                return Err(BakeError::Cancelled);
            }
            let mut sample = [[0.0f32; 3]; BakedMap::ALL.len()];
            for x in 0..w {
                let mut acc = [[0.0f32; 3]; BakedMap::ALL.len()];
                let mut n = 0u32;
                for i in subsamples(r, x) {
                    let (Some(surf), Some(&bary)) = (g.surface(req.mesh, i), g.bary.get(i)) else { continue };
                    // Seed by the global supersample index so strips don't change the noise.
                    ctx.evaluate(&maps, &surf, bary, (sy0 * sw + i) as u64, &mut sample);
                    for (a, v) in acc.iter_mut().zip(&sample) {
                        a.iter_mut().zip(v).for_each(|(a, v)| *a += v);
                    }
                    n += 1;
                }
                if let Some(c) = cov.get_mut(x) {
                    *c = n > 0;
                }
                for ((m, dst), a) in maps.iter().zip(row.iter_mut()).zip(&acc) {
                    let comps = m.components();
                    let mean = a.map(|a| a / n.max(1) as f32);
                    let v = if n == 0 || mean.iter().any(|v| !v.is_finite()) { m.neutral() } else { mean };
                    if let Some(d) = dst.get_mut(x * comps..(x + 1) * comps) {
                        d.iter_mut().zip(v).for_each(|(d, v)| *d = v);
                    }
                }
            }
            let d = rows_done.fetch_add(1, Relaxed) + 1;
            progress.done.fetch_max((d.saturating_mul(10_000) / total).min(10_000) as u32, Relaxed);
            Ok(())
        })?;
    }

    let mut out: BTreeMap<BakedMap, Vec<f32>> = maps.into_iter().zip(bufs).collect();
    if from_normal {
        let normal = if s.maps.contains(&BakedMap::Normal) { out.get(&BakedMap::Normal).cloned() } else { out.remove(&BakedMap::Normal) };
        if let (Some(normal), Some(curv)) = (normal, out.get_mut(&BakedMap::Curvature)) {
            maps::curvature_from_normal(&normal, &covered, w, tile_res, s.curvature_strength, curv);
        }
    }
    if s.dilation > 0 {
        // Padding stays inside each UV tile.
        let pad = DilationMap::in_blocks(&covered, w as u32, h as u32, Some(s.dilation), s.resolution);
        for (m, data) in out.iter_mut() {
            pad.apply(data, m.components());
        }
    }
    progress.done.store(10_000, Relaxed);
    Ok(BakeOutput { resolution: w as u32, height: h as u32, maps: out })
}
