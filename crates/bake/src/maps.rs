//! Per-sample evaluation of every baked map.

use std::collections::HashMap;
use std::f32::consts::TAU;
use std::ops::{Add, Mul};

use texturecraft_core::{BakeMatch, BakeSettings, BakedMap};
use texturecraft_geom::glam::{Vec3, Vec4};
use texturecraft_geom::{Bvh, Mesh, Surface, surface_at};

use crate::{BakeError, BakeRequest};

/// Triangles that belong to no submesh (never matched by name).
const NO_KEY: u32 = u32::MAX;

/// A mesh the bakers sample: the low poly, or the high poly detail is transferred from.
struct Source<'a> {
    mesh: &'a Mesh,
    bvh: &'a Bvh,
    /// Per-vertex curvature offset from 0.5 (empty unless mesh curvature is baked).
    curvature: Vec<f32>,
    /// Submesh (part) index of each triangle.
    part: Vec<u32>,
    /// Match key of each triangle ([`BakeMatch::ByMeshName`] only, else empty).
    key: Vec<u32>,
}

impl<'a> Source<'a> {
    fn new(mesh: &'a Mesh, bvh: &'a Bvh, s: &BakeSettings, diag: f32, suffix: &str, keys: &mut HashMap<String, u32>) -> Source<'a> {
        let mesh_curvature = s.maps.contains(&BakedMap::Curvature) && !s.curvature_from_normal;
        let tris = mesh.triangle_count();
        let mut part = vec![NO_KEY; tris];
        for (i, sm) in mesh.submeshes.iter().enumerate() {
            let (a, b) = ((sm.start / 3) as usize, (sm.start.saturating_add(sm.count) / 3) as usize);
            part.get_mut(a.min(b)..b.min(tris)).unwrap_or_default().fill(i as u32);
        }
        let key = match s.matching {
            BakeMatch::Always => Vec::new(),
            BakeMatch::ByMeshName => {
                let ids: Vec<u32> = mesh.submeshes.iter().map(|sm| match_key(&sm.name, suffix, keys)).collect();
                part.iter().map(|p| ids.get(*p as usize).copied().unwrap_or(NO_KEY)).collect()
            }
        };
        Source { mesh, bvh, curvature: if mesh_curvature { vertex_curvature(mesh, diag, s.curvature_strength) } else { Vec::new() }, part, key }
    }

    fn curvature(&self, s: &Surface, bary: [f32; 2]) -> f32 {
        (0.5 + interpolate(self.mesh, &self.curvature, s.tri, bary).unwrap_or(0.0)).clamp(0.0, 1.0)
    }

    fn color(&self, s: &Surface, bary: [f32; 2]) -> Vec4 {
        interpolate(self.mesh, &self.mesh.colors, s.tri, bary).unwrap_or(Vec4::ONE)
    }
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

/// The high-poly surface found for a low-poly sample, and where along the ray.
struct Transfer {
    surf: Surface,
    bary: [f32; 2],
    /// Hit distance from the ray origin, and the origin's distance in front of the low poly.
    t: f32,
    front: f32,
}

/// Everything a sample needs, computed once per bake.
pub(crate) struct Ctx<'a> {
    low: Source<'a>,
    high: Option<Source<'a>>,
    cage: Option<&'a Mesh>,
    /// Normals averaged over coincident vertices (rays don't split at hard edges); empty when off.
    ray_normals: Vec<Vec3>,
    lo: Vec3,
    extent: Vec3,
    /// Ray offset off the surface.
    eps: f32,
    /// AO / thickness ray length.
    dmax: f32,
    frontal: f32,
    rear: f32,
    rays: u32,
    direction: Vec3,
    curvature_from_normal: bool,
    occlusion: bool,
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
        let low = Source::new(req.mesh, req.bvh, s, diag, &s.low_suffix, &mut keys);
        let high = req.high.map(|h| Source::new(h.mesh, h.bvh, s, diag, &s.high_suffix, &mut keys));
        let ray_normals = match s.average_normals && high.is_some() {
            true => {
                let (weld, _, nrm) = welded(req.mesh, diag);
                weld.iter().map(|w| nrm.get(*w as usize).copied().unwrap_or(Vec3::Y)).collect()
            }
            false => Vec::new(),
        };
        Ok(Ctx {
            low,
            high,
            cage: req.cage,
            ray_normals,
            lo,
            extent: hi - lo,
            eps: 1e-4 * diag,
            dmax: s.max_distance * diag,
            frontal: s.max_frontal * diag,
            rear: s.max_rear * diag,
            rays: s.rays,
            direction: Vec3::from(s.direction).try_normalize().unwrap_or(Vec3::Y),
            curvature_from_normal: s.curvature_from_normal,
            occlusion: s.maps.contains(&BakedMap::AmbientOcclusion) || s.maps.contains(&BakedMap::BentNormal),
        })
    }

    /// Every map of `maps` at one low-poly surface sample, into `out` (scalars fill all three).
    pub(crate) fn evaluate(&self, maps: &[BakedMap], s: &Surface, bary: [f32; 2], seed: u64, out: &mut [[f32; 3]]) {
        let transfer = self.high.as_ref().and_then(|h| self.transfer(h, s, bary));
        // The surface the bakers read: the high poly where a ray found it, else the low poly.
        let (src, surf, sb) = match (&self.high, &transfer) {
            (Some(h), Some(tr)) => (h, &tr.surf, tr.bary),
            _ => (&self.low, s, bary),
        };
        let missed = self.high.is_some() && transfer.is_none();
        let (ao, bent) = if self.occlusion { self.occlusion(src, surf, seed) } else { (1.0, surf.nrm) };
        for (&map, o) in maps.iter().zip(out.iter_mut()) {
            *o = match map {
                BakedMap::AmbientOcclusion => [ao; 3],
                BakedMap::BentNormal => pack(bent),
                BakedMap::Thickness => [self.thickness(src, surf, seed); 3],
                BakedMap::Curvature if self.curvature_from_normal => [0.5; 3],
                BakedMap::Curvature => [src.curvature(surf, sb); 3],
                BakedMap::WorldNormal => pack(surf.nrm),
                BakedMap::Position => ((surf.pos - self.lo) / self.extent.max(Vec3::splat(1e-20))).clamp(Vec3::ZERO, Vec3::ONE).to_array(),
                BakedMap::Id => material_color(src.part.get(surf.tri as usize).copied().unwrap_or(0) as usize),
                BakedMap::Normal => transfer.as_ref().map_or(map.neutral(), |tr| tangent_normal(s, tr.surf.nrm)),
                BakedMap::Height => [transfer.as_ref().map_or(0.5, |tr| (0.5 + (tr.front - tr.t) / (tr.front + self.rear)).clamp(0.0, 1.0)); 3],
                BakedMap::Opacity if missed => [0.0; 3],
                BakedMap::Opacity => [src.color(surf, sb).w; 3],
                BakedMap::ColorMap if missed => map.neutral(),
                BakedMap::ColorMap => src.color(surf, sb).truncate().to_array(),
                BakedMap::WorldDirection => [surf.nrm.dot(self.direction) * 0.5 + 0.5; 3],
            };
        }
    }

    /// Cast from the cage (or `frontal` in front of the low poly) back through it to `rear`
    /// behind, and take the closest high-poly surface (of the matching part).
    fn transfer(&self, high: &Source, s: &Surface, bary: [f32; 2]) -> Option<Transfer> {
        let n = interpolate(self.low.mesh, &self.ray_normals, s.tri, bary).and_then(Vec3::try_normalize).unwrap_or(s.nrm);
        let cage = self.cage.and_then(|c| interpolate(c, &c.positions, s.tri, bary)).map(|c| s.pos - c).filter(|v| v.length() > self.eps);
        let (o, d, front) = match cage {
            Some(v) => (s.pos - v, v.normalize(), v.length()),
            None => (s.pos + n * self.frontal, -n, self.frontal),
        };
        let tmax = front + self.rear;
        let hit = match self.low.key.get(s.tri as usize) {
            Some(&k) => high.bvh.closest_filtered(o, d, tmax, &|t| k != NO_KEY && high.key.get(t as usize) == Some(&k))?,
            None => high.bvh.closest(o, d, tmax)?,
        };
        let surf = surface_at(high.mesh, hit.tri, hit.u, hit.v)?;
        Some(Transfer { surf, bary: [hit.u, hit.v], t: hit.t, front })
    }

    /// Geometric normal flipped to the side of the shading normal.
    fn geo(s: &Surface) -> Vec3 {
        if s.geo_nrm.dot(s.nrm) < 0.0 { -s.geo_nrm } else { s.geo_nrm }
    }

    /// Call `f` with cosine-weighted directions around `axis` (Hammersley points with a
    /// per-sample Cranley–Patterson rotation), skipping those below the plane of `geo`.
    fn directions(&self, axis: Vec3, geo: Vec3, seed: u64, mut f: impl FnMut(Vec3)) {
        let h = splitmix64(seed);
        let (r1, r2) = ((h >> 40) as f32 / (1u64 << 24) as f32, ((h >> 16) & 0xff_ffff) as f32 / (1u64 << 24) as f32);
        let (t, b) = axis.any_orthonormal_pair();
        for i in 0..self.rays {
            let u1 = (i as f32 / self.rays as f32 + r1).fract();
            let u2 = (i.reverse_bits() as f32 / 4_294_967_296.0 + r2).fract();
            let (phi, r) = (TAU * u1, u2.sqrt());
            let d = t * (r * phi.cos()) + b * (r * phi.sin()) + axis * (1.0 - u2).max(0.0).sqrt();
            if d.dot(geo) > 0.0 {
                f(d);
            }
        }
    }

    /// Ambient occlusion (1 = open, 0 = fully occluded) and the bent normal (mean unoccluded
    /// direction), from one set of rays.
    fn occlusion(&self, src: &Source, s: &Surface, seed: u64) -> (f32, Vec3) {
        let geo = Self::geo(s);
        let o = s.pos + geo * self.eps;
        let (mut occ, mut bent, mut n) = (0.0f32, Vec3::ZERO, 0u32);
        self.directions(s.nrm, geo, seed, |d| {
            let v = src.bvh.closest(o, d, self.dmax).map_or(0.0, |h| 1.0 - (h.t / self.dmax).powi(2));
            occ += v;
            bent += d * (1.0 - v);
            n += 1;
        });
        let ao = if n == 0 { 1.0 } else { (1.0 - occ / n as f32).clamp(0.0, 1.0) };
        (ao, bent.try_normalize().unwrap_or(s.nrm))
    }

    /// Mean distance through the mesh along inward rays, relative to the max distance (1 = thick).
    fn thickness(&self, src: &Source, s: &Surface, seed: u64) -> f32 {
        let geo = Self::geo(s);
        let o = s.pos - geo * self.eps;
        let (mut sum, mut n) = (0.0f32, 0u32);
        self.directions(-s.nrm, -geo, seed, |d| {
            sum += src.bvh.closest(o, d, self.dmax).map_or(self.dmax, |h| h.t);
            n += 1;
        });
        if n == 0 { 1.0 } else { (sum / n as f32 / self.dmax).clamp(0.0, 1.0) }
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
/// texels (convex = above 0.5). Writes covered texels of `out` (one float per texel).
/// `res` is the image width; the sampling step follows the resolution of one UV tile.
pub(crate) fn curvature_from_normal(normal: &[f32], covered: &[bool], res: usize, tile_res: usize, strength: f32, out: &mut [f32]) {
    use rayon::prelude::*;
    let r = (tile_res / 512).max(1);
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
    for t in mesh.indices.chunks_exact(3) {
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
