//! Per-sample evaluation of every baked map.

use std::collections::HashMap;
use std::ops::{Add, Mul};
use std::sync::Arc;

use texturecraft_core::{BakeMatch, BakeSettings, BakedMap, DirectionMode, PositionMode, RayDistribution, RaySettings, SelfOcclusion};
use texturecraft_geom::glam::{Vec2, Vec3, Vec4};
use texturecraft_geom::{BaseColor, Bvh, Hit, Mesh, Surface, surface_at};
use texturecraft_texel::{linear_to_srgb, srgb_to_linear};

use crate::rays::{ANY, Hemisphere, Query, Ray, RayKind, TraceMesh};
use crate::skew::Prepared;
use crate::{BakeError, BakeRequest};

/// A mesh the bakers sample: the low poly, or the high poly detail is transferred from.
struct Source<'a> {
    mesh: &'a Mesh,
    bvh: &'a Bvh,
    /// Per-vertex curvature offset from 0.5 (empty unless mesh curvature is baked).
    curvature: Vec<f32>,
    /// Submesh (part) index of each triangle ([`ANY`] outside every part).
    part: Vec<u32>,
    /// Match key of each triangle: its part name without the low / high suffix.
    key: Vec<u32>,
    /// Base color per material (empty: vertex colors only).
    base_color: &'a [Option<Arc<BaseColor>>],
}

impl<'a> Source<'a> {
    fn new(
        mesh: &'a Mesh,
        bvh: &'a Bvh,
        base_color: &'a [Option<Arc<BaseColor>>],
        s: &BakeSettings,
        diag: f32,
        suffix: &str,
        keys: &mut HashMap<String, u32>,
    ) -> Source<'a> {
        let mesh_curvature = s.maps.contains(&BakedMap::Curvature) && !s.curvature_from_normal;
        let tris = mesh.triangle_count();
        let mut part = vec![ANY; tris];
        for (i, sm) in mesh.submeshes.iter().enumerate() {
            let (a, b) = ((sm.start / 3) as usize, (sm.start.saturating_add(sm.count) / 3) as usize);
            part.get_mut(a.min(b)..b.min(tris)).unwrap_or_default().fill(i as u32);
        }
        let ids: Vec<u32> = mesh.submeshes.iter().map(|sm| match_key(&sm.name, suffix, keys)).collect();
        let key = part.iter().map(|p| ids.get(*p as usize).copied().unwrap_or(ANY)).collect();
        let curvature = if mesh_curvature { vertex_curvature(mesh, diag, s.curvature_strength) } else { Vec::new() };
        Source { mesh, bvh, curvature, part, key, base_color }
    }

    fn trace(&self) -> TraceMesh<'_> {
        TraceMesh { mesh: self.mesh, bvh: self.bvh, keys: &self.key }
    }

    fn curvature(&self, s: &Surface, bary: [f32; 2]) -> f32 {
        (0.5 + interpolate(self.mesh, &self.curvature, s.tri, bary).unwrap_or(0.0)).clamp(0.0, 1.0)
    }

    /// Vertex color times the material's base color (color sRGB-encoded like a base color
    /// texture, alpha linear).
    fn color(&self, s: &Surface, bary: [f32; 2]) -> Vec4 {
        let vertex = interpolate(self.mesh, &self.mesh.colors, s.tri, bary).unwrap_or(Vec4::ONE);
        let material = self.part.get(s.tri as usize).and_then(|p| self.mesh.submeshes.get(*p as usize)).map(|sm| sm.material);
        match material.and_then(|m| self.base_color.get(m)?.as_deref()) {
            Some(b) => vertex * base_color(b, s.uv),
            None => vertex,
        }
    }
}

/// `b` at `uv` (bilinear in linear light, wrapped) times its factor: color sRGB-encoded,
/// alpha linear.
fn base_color(b: &BaseColor, uv: Vec2) -> Vec4 {
    let tex = if b.texels.is_empty() || !uv.is_finite() {
        Vec4::ONE
    } else {
        let (w, h) = (i64::from(b.width.max(1)), i64::from(b.height.max(1)));
        let texel = |x: i64, y: i64| {
            let t = b.texels.get((y.rem_euclid(h) * w + x.rem_euclid(w)) as usize).copied().unwrap_or([255; 4]);
            let c = |k: usize| f32::from(t[k]) / 255.0;
            Vec4::new(srgb_to_linear(c(0)), srgb_to_linear(c(1)), srgb_to_linear(c(2)), c(3))
        };
        let (fx, fy) = (uv.x * w as f32 - 0.5, uv.y * h as f32 - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let (x, y) = (x0 as i64, y0 as i64);
        texel(x, y).lerp(texel(x + 1, y), tx).lerp(texel(x, y + 1).lerp(texel(x + 1, y + 1), tx), ty)
    };
    let c = tex * b.factor;
    Vec4::new(linear_to_srgb(c.x), linear_to_srgb(c.y), linear_to_srgb(c.z), c.w)
}

/// The part name without its low / high suffix (case-insensitive), as a shared id.
fn match_key(name: &str, suffix: &str, keys: &mut HashMap<String, u32>) -> u32 {
    let name = name.trim().to_lowercase();
    let suffix = suffix.trim().to_lowercase();
    let base = name.strip_suffix(suffix.as_str()).filter(|_| !suffix.is_empty()).unwrap_or(&name).to_string();
    let next = keys.len() as u32;
    *keys.entry(base).or_insert(next)
}

/// Barycentric interpolation of a per-vertex attribute over triangle `tri`.
fn interpolate<T: Copy + Mul<f32, Output = T> + Add<Output = T>>(mesh: &Mesh, v: &[T], tri: u32, [b1, b2]: [f32; 2]) -> Option<T> {
    let [a, b, c] = mesh.tri(tri as usize)?;
    Some(*v.get(a)? * (1.0 - b1 - b2) + *v.get(b)? * b1 + *v.get(c)? * b2)
}

/// A covered low-poly supersample of a batch.
pub(crate) struct Sample {
    /// Output column.
    pub x: usize,
    pub surf: Surface,
    pub bary: [f32; 2],
    pub seed: u64,
}

/// The high-poly surface found for a low-poly sample, and where along the ray.
pub(crate) struct Transfer {
    surf: Surface,
    bary: [f32; 2],
    /// Hit distance from the ray origin, and the origin's distance in front of the low poly.
    t: f32,
    front: f32,
}

/// A sample's secondary-ray results.
pub(crate) struct Traced {
    pub ao: f32,
    /// None when bent normals are not baked.
    pub bent: Option<Vec3>,
    pub thickness: f32,
}

/// Everything a sample needs, computed once per bake.
pub(crate) struct Ctx<'a> {
    low: Source<'a>,
    high: Option<Source<'a>>,
    cage: Option<&'a Mesh>,
    skew: Option<Prepared<'a>>,
    /// Normals averaged over coincident vertices (rays don't split at hard edges); empty when off.
    ray_normals: Vec<Vec3>,
    lo: Vec3,
    extent: Vec3,
    /// Ray offset off the surface.
    eps: f32,
    frontal: f32,
    rear: f32,
    by_name: bool,
    direction: Vec3,
    direction_mode: DirectionMode,
    position_mode: PositionMode,
    curvature_from_normal: bool,
    /// Secondary-ray passes of the requested maps.
    pub ao: Option<Hemisphere>,
    pub bent: Option<Hemisphere>,
    pub thickness: Option<Hemisphere>,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(req: &BakeRequest<'a>) -> Result<Ctx<'a>, BakeError> {
        let s = req.settings;
        let (lo, hi) = req.mesh.bounds();
        let diag = (hi - lo).length().max(1e-6);
        if !diag.is_finite() {
            return Err(BakeError::Invalid("mesh bounds are too large".into()));
        }
        let mut keys = HashMap::new();
        let low = Source::new(req.mesh, req.bvh, &[], s, diag, &s.low_suffix, &mut keys);
        let high = req.high.map(|h| Source::new(h.mesh, h.bvh, h.base_color, s, diag, &s.high_suffix, &mut keys));
        let ray_normals = match s.average_normals && high.is_some() {
            true => {
                let (weld, _, nrm) = welded(req.mesh, diag);
                weld.iter().map(|w| nrm.get(*w as usize).copied().unwrap_or(Vec3::Y)).collect()
            }
            false => Vec::new(),
        };
        let skew = req.skew.map(|m| Prepared::new(m, req.mesh, diag)).transpose()?;
        let eps = 1e-4 * diag;
        let pass = |map: BakedMap, r: &RaySettings, kind: RayKind| {
            let half = (r.spread.clamp(0.0, 180.0) * 0.5).to_radians();
            let occlusion = kind == RayKind::Occlusion;
            s.maps.contains(&map).then_some(Hemisphere {
                kind,
                rays: r.rays,
                dmax: r.max_distance * diag,
                eps,
                cos_max: half.cos(),
                sin_max: half.sin(),
                uniform: r.distribution == RayDistribution::Uniform,
                ignore_backface: occlusion && s.ao_ignore_backface,
                same_key: occlusion && s.ao_self_occlusion == SelfOcclusion::SameMeshName,
            })
        };
        Ok(Ctx {
            low,
            high,
            cage: req.cage,
            skew,
            ray_normals,
            lo,
            extent: hi - lo,
            eps,
            frontal: s.max_frontal * diag,
            rear: s.max_rear * diag,
            by_name: s.matching == BakeMatch::ByMeshName,
            direction: Vec3::from(s.direction).try_normalize().unwrap_or(Vec3::Y),
            direction_mode: s.direction_mode,
            position_mode: s.position_mode,
            curvature_from_normal: s.curvature_from_normal,
            ao: pass(BakedMap::AmbientOcclusion, &s.ao, RayKind::Occlusion),
            bent: pass(BakedMap::BentNormal, &s.bent_normal, RayKind::Occlusion),
            thickness: pass(BakedMap::Thickness, &s.thickness, RayKind::Thickness),
        })
    }

    pub(crate) fn has_high(&self) -> bool {
        self.high.is_some()
    }

    /// Whether the bake casts any ray.
    pub(crate) fn casts_rays(&self) -> bool {
        self.has_high() || self.ao.is_some() || self.bent.is_some() || self.thickness.is_some()
    }

    /// The meshes rays are cast against: the low poly, then the high poly.
    pub(crate) fn trace_meshes(&self) -> Vec<TraceMesh<'_>> {
        std::iter::once(&self.low).chain(&self.high).map(Source::trace).collect()
    }

    /// The transfer ray of a low-poly sample: from the cage (or `frontal` in front of the low
    /// poly) back through it to `rear` behind, onto the matching high-poly part; and the
    /// origin's distance in front of the low poly. Parts without a key cast nothing when
    /// matched by name.
    pub(crate) fn transfer_ray(&self, s: &Surface, bary: [f32; 2]) -> (Ray, f32) {
        let n = interpolate(self.low.mesh, &self.ray_normals, s.tri, bary).and_then(Vec3::try_normalize).unwrap_or(s.nrm);
        let cage = self.cage.and_then(|c| interpolate(c, &c.positions, s.tri, bary)).map(|c| s.pos - c).filter(|v| v.length() > self.eps);
        let (origin, dir, front) = match cage {
            Some(v) => (s.pos - v, v.normalize(), v.length()),
            None => {
                // Skew correction turns the averaged direction towards the base normal.
                let up = match &self.skew {
                    Some(k) => k.correct(self.low.mesh, s, n),
                    None => n,
                };
                (s.pos + up * self.frontal, -up, self.frontal)
            }
        };
        let key = if self.by_name { self.low.key.get(s.tri as usize).copied().unwrap_or(ANY) } else { ANY };
        let tmax = if self.by_name && key == ANY { 0.0 } else { front + self.rear };
        (Ray { origin, dir, tmax, key }, front)
    }

    /// The high-poly surface a transfer ray hit.
    pub(crate) fn transfer(&self, hit: Option<Hit>, front: f32) -> Option<Transfer> {
        let (high, hit) = (self.high.as_ref()?, hit?);
        let surf = surface_at(high.mesh, hit.tri, hit.u, hit.v)?;
        Some(Transfer { surf, bary: [hit.u, hit.v], t: hit.t, front })
    }

    /// The secondary-ray query of a sample: on the high poly where the transfer found it.
    pub(crate) fn query(&self, transfer: Option<&Transfer>, s: &Surface, seed: u64) -> Query {
        let (src, surf, mesh) = match (&self.high, transfer) {
            (Some(h), Some(tr)) => (h, &tr.surf, 1),
            _ => (&self.low, s, 0),
        };
        let h = splitmix64(seed);
        let rot = [(h >> 40) as f32 / (1u64 << 24) as f32, ((h >> 16) & 0xff_ffff) as f32 / (1u64 << 24) as f32];
        // The geometric normal on the side of the shading normal.
        let geo = if surf.geo_nrm.dot(surf.nrm) < 0.0 { -surf.geo_nrm } else { surf.geo_nrm };
        Query { pos: surf.pos, nrm: surf.nrm, geo, rot, key: src.key.get(surf.tri as usize).copied().unwrap_or(ANY), mesh }
    }

    /// Every map of `maps` at one low-poly sample, into `out` (scalars fill all three).
    pub(crate) fn evaluate(&self, maps: &[BakedMap], s: &Surface, bary: [f32; 2], transfer: Option<&Transfer>, traced: &Traced, out: &mut [[f32; 3]]) {
        // The surface the bakers read: the high poly where a ray found it, else the low poly.
        let (src, surf, sb) = match (&self.high, transfer) {
            (Some(h), Some(tr)) => (h, &tr.surf, tr.bary),
            _ => (&self.low, s, bary),
        };
        let missed = self.high.is_some() && transfer.is_none();
        for (&map, o) in maps.iter().zip(out.iter_mut()) {
            *o = match map {
                BakedMap::AmbientOcclusion => [traced.ao; 3],
                BakedMap::BentNormal => pack(traced.bent.unwrap_or(surf.nrm)),
                BakedMap::Thickness => [traced.thickness; 3],
                BakedMap::Curvature if self.curvature_from_normal => [0.5; 3],
                BakedMap::Curvature => [src.curvature(surf, sb); 3],
                BakedMap::WorldNormal => pack(surf.nrm),
                BakedMap::Position => self.position(surf.pos),
                BakedMap::Id => material_color(src.part.get(surf.tri as usize).copied().unwrap_or(0) as usize),
                BakedMap::Normal => transfer.map_or(map.neutral(), |tr| tangent_normal(s, tr.surf.nrm)),
                BakedMap::Height => [transfer.map_or(0.5, |tr| (0.5 + (tr.front - tr.t) / (tr.front + self.rear)).clamp(0.0, 1.0)); 3],
                BakedMap::Opacity if missed => [0.0; 3],
                BakedMap::Opacity => [src.color(surf, sb).w; 3],
                BakedMap::ColorMap if missed => map.neutral(),
                BakedMap::ColorMap => src.color(surf, sb).truncate().to_array(),
                BakedMap::WorldDirection => {
                    let f = surf.nrm.dot(self.direction);
                    [match self.direction_mode {
                        DirectionMode::Signed => f * 0.5 + 0.5,
                        DirectionMode::Positive => f.clamp(0.0, 1.0),
                    }; 3]
                }
            };
        }
    }

    /// `p` normalized to 0..1 over the low poly's bounding box or bounding sphere.
    fn position(&self, p: Vec3) -> [f32; 3] {
        let v = match self.position_mode {
            PositionMode::BoundingBox => (p - self.lo) / self.extent.max(Vec3::splat(1e-20)),
            PositionMode::BoundingSphere => (p - (self.lo + self.extent * 0.5)) / self.extent.length().max(1e-20) + 0.5,
        };
        v.clamp(Vec3::ZERO, Vec3::ONE).to_array()
    }
}

/// A unit vector packed to 0..1.
fn pack(v: Vec3) -> [f32; 3] {
    (v * 0.5 + 0.5).clamp(Vec3::ZERO, Vec3::ONE).to_array()
}

/// `n` in the tangent space of low-poly sample `s` (OpenGL: +Y up the image), packed to 0..1.
fn tangent_normal(s: &Surface, n: Vec3) -> [f32; 3] {
    let z = s.nrm;
    let t = (s.tangent.truncate() - z * z.dot(s.tangent.truncate())).try_normalize().unwrap_or_else(|| z.any_orthonormal_vector());
    let b = z.cross(t) * s.tangent.w;
    pack(Vec3::new(t.dot(n), b.dot(n), z.dot(n)))
}

/// Curvature from a baked tangent-space normal map: the divergence of its XY over `radius`
/// texels (convex = above 0.5; 0: one texel per 512 of a UV tile's resolution). Writes covered
/// texels of `out` (one float per texel). `res` is the image width.
pub(crate) fn curvature_from_normal(normal: &[f32], covered: &[bool], res: usize, tile_res: usize, radius: u32, strength: f32, out: &mut [f32]) {
    use rayon::prelude::*;
    let r = if radius > 0 { radius as usize } else { (tile_res / 512).max(1) };
    let n = |x: usize, y: usize, c: usize, fallback: f32| match covered.get(y * res + x) {
        Some(true) => normal.get((y * res + x) * 3 + c).copied().unwrap_or(fallback),
        _ => fallback,
    };
    out.par_chunks_mut(res).enumerate().for_each(|(y, row)| {
        for (x, v) in row.iter_mut().enumerate() {
            if covered.get(y * res + x) != Some(&true) {
                continue;
            }
            let (cx, cy) = (n(x, y, 0, 0.5), n(x, y, 1, 0.5));
            let (l, r_) = (n(x.saturating_sub(r), y, 0, cx), if x + r < res { n(x + r, y, 0, cx) } else { cx });
            let (u, d) = (n(x, y.saturating_sub(r), 1, cy), if y + r < res { n(x, y + r, 1, cy) } else { cy });
            // Image rows run down while +Y (OpenGL) points up: convex bumps tilt +Y above.
            *v = (0.5 + strength * ((r_ - l) + (u - d))).clamp(0.0, 1.0);
        }
    });
}

/// Position-welded copy of `mesh` (UV seams and hard edges share neighbours): the weld id of
/// every vertex, welded positions and their averaged normals.
fn welded(mesh: &Mesh, diag: f32) -> (Vec<u32>, Vec<Vec3>, Vec<Vec3>) {
    let q = diag * 1e-5;
    let mut ids: HashMap<[i64; 3], u32> = HashMap::new();
    let (mut pos, mut nrm) = (Vec::new(), Vec::<Vec3>::new());
    let weld: Vec<u32> = mesh
        .positions
        .iter()
        .zip(&mesh.normals)
        .map(|(p, n)| {
            let key = [p.x, p.y, p.z].map(|c| (c / q).round() as i64);
            let id = *ids.entry(key).or_insert_with(|| {
                pos.push(*p);
                nrm.push(Vec3::ZERO);
                (pos.len() - 1) as u32
            });
            if let Some(acc) = nrm.get_mut(id as usize) {
                *acc += *n;
            }
            id
        })
        .collect();
    let nrm = nrm.into_iter().map(|n| n.try_normalize().unwrap_or(Vec3::ZERO)).collect();
    (weld, pos, nrm)
}

/// Per-vertex curvature offset `−k · avg_edge · strength`, where `k` is the mean of
/// `n·(p_j − p_i)/|p_j − p_i|²` over the 1-ring of the welded mesh. Convex edges give a
/// positive offset.
fn vertex_curvature(mesh: &Mesh, diag: f32, strength: f32) -> Vec<f32> {
    let q = diag * 1e-5;
    let (weld, pos, nrm) = welded(mesh, diag);
    let (mut ksum, mut kcnt) = (vec![0.0f32; pos.len()], vec![0u32; pos.len()]);
    let (mut edge_sum, mut edge_cnt) = (0.0f64, 0u64);
    for t in mesh.indices.as_chunks::<3>().0 {
        for (a, b) in [(0, 1), (1, 2), (2, 0), (1, 0), (2, 1), (0, 2)] {
            let (Some(&wa), Some(&wb)) = (t.get(a).and_then(|v| weld.get(*v as usize)), t.get(b).and_then(|v| weld.get(*v as usize))) else { continue };
            let (Some(pa), Some(pb), Some(na)) = (pos.get(wa as usize), pos.get(wb as usize), nrm.get(wa as usize)) else { continue };
            let d = *pb - *pa;
            let l2 = d.length_squared();
            if wa == wb || l2 <= q * q {
                continue;
            }
            if let (Some(k), Some(c)) = (ksum.get_mut(wa as usize), kcnt.get_mut(wa as usize)) {
                *k += na.dot(d) / l2;
                *c += 1;
            }
            edge_sum += f64::from(l2.sqrt());
            edge_cnt += 1;
        }
    }
    let scale = (edge_sum / edge_cnt.max(1) as f64) as f32 * strength;
    weld.iter()
        .map(|w| {
            let i = *w as usize;
            let k = ksum.get(i).copied().unwrap_or(0.0) / kcnt.get(i).copied().unwrap_or(0).max(1) as f32;
            if k.is_finite() { -k * scale } else { 0.0 }
        })
        .collect()
}

/// A well-separated hue per part (golden-ratio steps around the HSV wheel).
fn material_color(part: usize) -> [f32; 3] {
    let h = ((part as f64 * 0.618_033_988_75 + 0.05).fract() * 6.0) as f32;
    let (s, v) = (0.7f32, 0.95f32);
    let f = h.fract();
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match h as u32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}
