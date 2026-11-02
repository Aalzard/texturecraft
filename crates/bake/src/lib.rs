//! # texturecraft-bake
//!
//! Mesh-map baking. [`bake`] rasterizes one texture set's UV layout with a supersampled
//! [`UvGBuffer`] and works through it in batches of samples: the high-poly transfer rays, then
//! the secondary rays of ambient occlusion, bent normals and thickness (each with its own
//! [`texturecraft_core::RaySettings`]) go to a [`rays::Tracer`], then every requested
//! [`BakedMap`] is evaluated per sample (rayon over rows), box-downsampled, and the UV islands
//! are padded with [`DilationMap`]. The rays run on the CPU ([`rays::CpuTracer`], the oracle) or
//! on a GPU tracer passed to [`bake_on`]. With a high poly, every map but position and
//! world-space direction reads the high-poly surface found along the cage / frontal-rear ray;
//! the color map multiplies its vertex colors by its materials' [`BaseColor`]s. Sampling is
//! deterministic, so the same request always gives the same pixels.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod maps;
pub mod rays;
mod skew;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

use rayon::prelude::*;
use texturecraft_core::{BakeSettings, BakedMap};
use texturecraft_geom::glam::{Vec2, Vec3};
use texturecraft_geom::{BaseColor, Bvh, Mesh, NO_TRI, UvGBuffer};
use texturecraft_texel::DilationMap;

use crate::rays::{CpuTracer, RayScene, Tracer};
pub use crate::skew::SkewMap;

/// Largest accepted output resolution (per axis).
pub const MAX_RESOLUTION: u32 = 8192;
/// Largest accepted ray count per texel (AO, thickness).
pub const MAX_RAYS: u32 = 4096;
/// Largest accepted output (texels over every UV tile).
pub const MAX_TEXELS: u64 = 1 << 27;
/// Supersampled texels rasterized at once: bounds the G-buffer at ~200 MB for any resolution × AA.
const STRIP_TEXELS: usize = 1 << 24;
/// Supersamples ray-cast per batch: bounds the per-sample buffers (~40 MB) and a GPU submission.
const BATCH: usize = 1 << 18;

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
    /// Base color per material of `mesh` (missing or None: white).
    pub base_color: &'a [Option<Arc<BaseColor>>],
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
    /// Skew correction of the high-poly transfer rays (see [`SkewMap`]); ignored with a cage mesh.
    pub skew: Option<&'a SkewMap>,
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
    /// Where the rays ran ([`Tracer::name`]).
    pub tracer: String,
}

#[derive(Debug, thiserror::Error)]
pub enum BakeError {
    #[error("bake cancelled")]
    Cancelled,
    #[error("invalid bake request: {0}")]
    Invalid(String),
    /// The ray tracer failed ([`bake_on`] then bakes on the CPU).
    #[error("ray tracer: {0}")]
    Tracer(String),
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
    for (map, r) in [(BakedMap::AmbientOcclusion, &s.ao), (BakedMap::Thickness, &s.thickness), (BakedMap::BentNormal, &s.bent_normal)] {
        if !s.maps.contains(&map) {
            continue;
        }
        if r.rays == 0 || r.rays > MAX_RAYS {
            return invalid(format!("{} ray count {} is not in 1..={MAX_RAYS}", map.label(), r.rays));
        }
        if !(r.max_distance.is_finite() && r.max_distance > 0.0) {
            return invalid(format!("{} max distance {} must be positive", map.label(), r.max_distance));
        }
        if !r.spread.is_finite() {
            return invalid(format!("{} spread angle is not finite", map.label()));
        }
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

/// Bake the maps in `req.settings.maps` for one material of the low-poly mesh, on the CPU.
pub fn bake(req: &BakeRequest, progress: &Progress) -> Result<BakeOutput, BakeError> {
    bake_in_strips(req, progress, STRIP_TEXELS, &CpuTracer)
}

/// [`bake`] with the rays cast by `tracer` (a GPU); when the tracer fails (no adapter, a mesh
/// too large for it, a lost device) the bake runs again on the CPU.
pub fn bake_on(req: &BakeRequest, progress: &Progress, tracer: &dyn Tracer) -> Result<BakeOutput, BakeError> {
    match bake_in_strips(req, progress, STRIP_TEXELS, tracer) {
        Err(BakeError::Tracer(e)) => {
            let mut out = bake(req, progress)?;
            out.tracer = format!("CPU ({} unavailable: {e})", tracer.name());
            Ok(out)
        }
        r => r,
    }
}

/// One output row of a strip: its slice of every map.
type OutRow<'a> = Vec<&'a mut [f32]>;

fn bake_in_strips(req: &BakeRequest, progress: &Progress, strip_texels: usize, tracer: &dyn Tracer) -> Result<BakeOutput, BakeError> {
    validate(req)?;
    let s = req.settings;
    let (tile_res, aa) = (s.resolution as usize, s.antialiasing as usize);
    let (w, h) = (tile_res * req.tiles.0 as usize, tile_res * req.tiles.1 as usize);
    let ctx = maps::Ctx::new(req)?;
    let tracer = if ctx.casts_rays() { tracer } else { &CpuTracer };
    let meshes = ctx.trace_meshes();
    let mut scene = tracer.scene(&meshes)?;
    progress.done.store(0, Relaxed);

    // Curvature from the normal map needs the normal map: baked alongside when not requested.
    let from_normal = s.curvature_from_normal && s.maps.contains(&BakedMap::Curvature);
    let mut maps: Vec<BakedMap> = s.maps.iter().copied().collect();
    if from_normal && !maps.contains(&BakedMap::Normal) {
        maps.push(BakedMap::Normal);
    }
    let mut bufs = maps.iter().map(|m| alloc(w * h * m.components(), 0.0f32)).collect::<Result<Vec<_>, _>>()?;
    let mut covered = alloc(w * h, false)?;
    let mut pad = None;
    let total = h.max(1) as u64;
    let mut rows_done = 0u64;
    let strip_err = || BakeError::Invalid("strip out of range".into());

    // Rasterize in horizontal strips of the supersampled image: a strip's G-buffer is built from
    // UVs shifted so that y = v*H - strip_top, which keeps memory bounded at 8K x 8 AA.
    let (sw, full_h) = (w * aa, h * aa);
    let strip_rows = (strip_texels / (sw * aa)).max(1).min(h);
    let batch_rows = (BATCH / (sw * aa)).max(1);
    let mut strip_mesh = Mesh { indices: req.mesh.indices.clone(), submeshes: req.mesh.submeshes.clone(), ..Mesh::default() };
    for y0 in (0..h).step_by(strip_rows) {
        let rows = strip_rows.min(h - y0);
        let (sy0, sh) = (y0 * aa, rows * aa);
        strip_mesh.uvs = req.mesh.uvs.iter().map(|uv| Vec2::new(uv.x, (uv.y * full_h as f32 - sy0 as f32) / sh as f32)).collect();
        let g = UvGBuffer::new(&strip_mesh, req.material, sw as u32, sh as u32);
        // A texel is covered when any of its supersamples is.
        let cov = covered.get_mut(y0 * w..(y0 + rows) * w).ok_or_else(strip_err)?;
        cov.par_chunks_mut(w).enumerate().for_each(|(r, row)| {
            for (x, c) in row.iter_mut().enumerate() {
                *c = (0..aa * aa).any(|k| g.tri.get((r * aa + k / aa) * sw + x * aa + k % aa).is_some_and(|t| *t != NO_TRI));
            }
        });

        let mut out_rows: Vec<OutRow> = (0..rows).map(|_| Vec::with_capacity(maps.len())).collect();
        for (m, data) in maps.iter().zip(bufs.iter_mut()) {
            let comps = m.components();
            let strip = data.get_mut(y0 * w * comps..(y0 + rows) * w * comps).ok_or_else(strip_err)?;
            for (row, chunk) in out_rows.iter_mut().zip(strip.chunks_mut(w * comps)) {
                row.push(chunk);
            }
        }
        let mut bake_strip = || {
            for (b, batch) in out_rows.chunks_mut(batch_rows).enumerate() {
                if progress.cancel.load(Relaxed) {
                    return Err(BakeError::Cancelled);
                }
                // Seed by the global supersample index so strips and batches don't change the noise.
                bake_rows(&ctx, &maps, req.mesh, &g, scene.as_mut(), batch, b * batch_rows, (w, aa, sy0 * sw))?;
                rows_done += batch.len() as u64;
                progress.done.store((rows_done.saturating_mul(10_000) / total).min(10_000) as u32, Relaxed);
            }
            Ok(())
        };
        if y0 + rows < h || s.dilation == 0 {
            bake_strip()?;
        } else {
            // The coverage is complete: find the padding (UV tile by UV tile) while the last strip bakes.
            let padding = || DilationMap::in_blocks(&covered, w as u32, h as u32, Some(s.dilation), s.resolution);
            let (p, baked) = rayon::join(padding, bake_strip);
            baked?;
            pad = Some(p);
        }
    }

    let mut out: BTreeMap<BakedMap, Vec<f32>> = maps.into_iter().zip(bufs).collect();
    if from_normal {
        let normal = if s.maps.contains(&BakedMap::Normal) { out.get(&BakedMap::Normal).cloned() } else { out.remove(&BakedMap::Normal) };
        if let (Some(normal), Some(curv)) = (normal, out.get_mut(&BakedMap::Curvature)) {
            maps::curvature_from_normal(&normal, &covered, w, tile_res, s.curvature_radius, s.curvature_strength, curv);
        }
    }
    if let Some(pad) = pad {
        for (m, data) in out.iter_mut() {
            pad.apply(data, m.components());
        }
    }
    progress.done.store(10_000, Relaxed);
    Ok(BakeOutput { resolution: w as u32, height: h as u32, maps: out, tracer: tracer.name() })
}

/// Bake output rows `r0..` of a strip's G-buffer `g` into `rows`: gather the covered
/// supersamples, cast their transfer and secondary rays in batches on `scene`, then evaluate
/// and average every map per texel. `seed0` is the strip's first supersample index.
#[allow(clippy::too_many_arguments)]
fn bake_rows(
    ctx: &maps::Ctx,
    maps: &[BakedMap],
    mesh: &Mesh,
    g: &UvGBuffer,
    scene: &mut dyn RayScene,
    rows: &mut [OutRow],
    r0: usize,
    (w, aa, seed0): (usize, usize, usize),
) -> Result<(), BakeError> {
    let sw = w * aa;
    let per_row: Vec<Vec<maps::Sample>> = (r0..r0 + rows.len())
        .into_par_iter()
        .map(|r| {
            let subsamples = (0..w).flat_map(|x| (0..aa * aa).map(move |k| (x, (r * aa + k / aa) * sw + x * aa + k % aa)));
            subsamples.filter_map(|(x, i)| Some(maps::Sample { x, surf: g.surface(mesh, i)?, bary: *g.bary.get(i)?, seed: (seed0 + i) as u64 })).collect()
        })
        .collect();
    // Index of each row's first sample in the batch arrays.
    let starts: Vec<usize> = per_row
        .iter()
        .scan(0, |n, r| {
            *n += r.len();
            Some(*n - r.len())
        })
        .collect();
    let samples = || per_row.par_iter().flat_map_iter(|r| r.iter());

    let transfers = match ctx.has_high() {
        true => {
            let (rays, fronts): (Vec<rays::Ray>, Vec<f32>) = samples().map(|p| ctx.transfer_ray(&p.surf, p.bary)).unzip();
            let hits = scene.closest(1, &rays)?;
            hits.into_par_iter().zip(fronts).map(|(hit, front)| ctx.transfer(hit, front)).collect()
        }
        false => Vec::new(),
    };
    let transfer = |k: usize| transfers.get(k).and_then(Option::as_ref);
    let queries: Vec<rays::Query> = match ctx.ao.is_some() || ctx.bent.is_some() || ctx.thickness.is_some() {
        true => {
            per_row.par_iter().zip(&starts).flat_map_iter(|(r, &first)| r.iter().zip(first..)).map(|(p, k)| ctx.query(transfer(k), &p.surf, p.seed)).collect()
        }
        false => Vec::new(),
    };
    // Bent normals come with the occlusion rays when they share the settings.
    let own_bent = ctx.bent.filter(|_| ctx.bent != ctx.ao);
    let passes: Vec<rays::Hemisphere> = [ctx.ao, own_bent, ctx.thickness].into_iter().flatten().collect();
    let mut results = scene.hemispheres(&passes, &queries)?.into_iter();
    let mut next = |on: bool| if on { results.next().unwrap_or_default() } else { Vec::new() };
    let (ao, bent_own, thickness) = (next(ctx.ao.is_some()), next(own_bent.is_some()), next(ctx.thickness.is_some()));
    let bent = if own_bent.is_some() { &bent_own } else { &ao };

    rows.par_iter_mut().zip(&per_row).zip(&starts).for_each(|((row, samples), &first)| {
        let mut sample = [[0.0f32; 3]; BakedMap::ALL.len()];
        let mut j = 0;
        for x in 0..w {
            let mut acc = [[0.0f32; 3]; BakedMap::ALL.len()];
            let mut n = 0u32;
            while let Some(p) = samples.get(j).filter(|p| p.x == x) {
                let k = first + j;
                let traced = maps::Traced {
                    ao: ao.get(k).map_or(1.0, |v| v[0]),
                    bent: bent.get(k).map(|v| Vec3::new(v[1], v[2], v[3])),
                    thickness: thickness.get(k).map_or(1.0, |v| v[0]),
                };
                ctx.evaluate(maps, &p.surf, p.bary, transfer(k), &traced, &mut sample);
                for (a, v) in acc.iter_mut().zip(&sample) {
                    a.iter_mut().zip(v).for_each(|(a, v)| *a += v);
                }
                n += 1;
                j += 1;
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
    });
    Ok(())
}
