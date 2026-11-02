//! Per-sample evaluation of every baked map.

use std::collections::HashMap;
use std::ops::{Add, Mul};
use std::sync::Arc;

use texturecraft_core::{BakeMatch, BakeSettings, BakedMap, DirectionMode, PositionMode, RayDistribution, RaySettings, SelfOcclusion};
use texturecraft_geom::glam::{Vec2, Vec3, Vec4};
use texturecraft_geom::{BaseColor, Bvh, Hit, Mesh, Surface, surface_at};
use texturecraft_texel::{linear_to_srgb, srgb_to_linear};

use rayon::prelude::*;

use crate::rays::{ANY, Hemisphere, Query, Ray, RayKind, RayScene, TraceMesh};
use crate::skew::Prepared;
use crate::{BakeError, BakeRequest};

/// A mesh the bakers sample: the low poly, or the high poly detail is transferred from.
struct Source<'a> {
    mesh: &'a Mesh,
    bvh: &'a Bvh,
    /// Submesh (part) index of each triangle ([`ANY`] outside every part).
    part: Vec<u32>,
    /// Match key of each triangle: its part name without the low / high suffix.
    key: Vec<u32>,
    /// Base color per material (empty: vertex colors only).
    base_color: &'a [Option<Arc<BaseColor>>],
}

impl<'a> Source<'a> {
    fn new(mesh: &'a Mesh, bvh: &'a Bvh, base_color: &'a [Option<Arc<BaseColor>>], suffix: &str, keys: &mut HashMap<String, u32>) -> Source<'a> {
        let tris = mesh.triangle_count();
        let mut part = vec![ANY; tris];
        for (i, sm) in mesh.submeshes.iter().enumerate() {
            let (a, b) = ((sm.start / 3) as usize, (sm.start.saturating_add(sm.count) / 3) as usize);
            part.get_mut(a.min(b)..b.min(tris)).unwrap_or_default().fill(i as u32);
        }
        let ids: Vec<u32> = mesh.submeshes.iter().map(|sm| match_key(&sm.name, suffix, keys)).collect();
        let key = part.iter().map(|p| ids.get(*p as usize).copied().unwrap_or(ANY)).collect();
        Source { mesh, bvh, part, key, base_color }
    }

    fn trace(&self) -> TraceMesh<'_> {
        TraceMesh { mesh: self.mesh, bvh: self.bvh, keys: &self.key }
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

/// Mesh curvature from short rays around each sample, in batches, so the GPU casts them when there
/// is one. The result does not depend on how big the triangles are, and it reads across UV seams and
/// between separate parts. Line radii are in texels of the low poly triangle under the sample, so a
/// line is as wide on a small densely unwrapped part as on a big plate.
struct Curvature {
    on: bool,
    strength: f32,
    /// Sampling radius of the walls, and of the smooth normals, in texels.
    walls: f32,
    spread: f32,
    /// World size of a texel on triangles without usable UVs.
    fallback: f32,
    /// World size of one texel on each low poly triangle; empty when off.
    texels: Vec<f32>,
    /// Cosine and sine of each direction of a ring.
    ring: [(f32, f32); CURVATURE_RAYS],
    /// Read from the low poly itself, and from a high poly.
    low: Tuning,
    high: Tuning,
}

/// What differs between reading the low poly and reading a high poly.
struct Tuning {
    convex: f32,
    /// World radius of the true curvature: wider than the facets of a dense mesh, so it averages
    /// across them, and the same at any map size.
    true_radius: f32,
    true_strength: f32,
}

/// Rays per ring around a sample.
const CURVATURE_RAYS: usize = 8;
/// Samples whose curvature rays are cast at once: bounds their memory (about 2 MB of rays and hits
/// per thousand samples) and a GPU submission, at any resolution and antialiasing.
const CURVATURE_CHUNK: usize = 1 << 15;
/// Rings: wall above, wall below, smooth normals, true curvature.
const CURVATURE_RINGS: usize = 4;
/// Sampling radius of the walls, in texels of the low poly triangle.
const CURVATURE_WALLS: f32 = 1.5;
/// Weight of the concave walls, and of the convex edges from the low poly itself and a high poly.
const CURVATURE_CONCAVE: f32 = 1.5;
const CURVATURE_CONVEX: f32 = 1.5;
const CURVATURE_CONVEX_HIGH: f32 = 0.5;
/// Radius of the true curvature, as a fraction of the model size, and its strength, from the low
/// poly itself and from a high poly. A high poly's small radius keeps a line along a whole edge and
/// its dense detail from wearing in patches.
const CURVATURE_TRUE_RADIUS: f32 = 0.003;
const CURVATURE_TRUE: f32 = 1.3;
const CURVATURE_TRUE_RADIUS_HIGH: f32 = 0.0007;
const CURVATURE_TRUE_HIGH: f32 = 1.6;
/// How much the shading normal must turn (1 − cos) for a corner to count as hard.
const CURVATURE_HARD_EDGE: f32 = 0.05;
/// Map value below which true curvature counts as gently round.
const CURVATURE_GENTLE: f32 = 0.1;

fn triangle_normal(mesh: &Mesh, tri: u32) -> Option<Vec3> {
    let [a, b, c] = mesh.tri(tri as usize)?;
    let p = |i: usize| mesh.positions.get(i).copied();
    (p(b)? - p(a)?).cross(p(c)? - p(a)?).try_normalize()
}

/// The geometric normal on the side of the shading normal, and two directions along the surface.
fn surface_frame(surf: &Surface) -> (Vec3, Vec3, Vec3) {
    let geo = if surf.geo_nrm.dot(surf.nrm) < 0.0 { -surf.geo_nrm } else { surf.geo_nrm };
    let (t, b) = geo.any_orthonormal_pair();
    (geo, t, b)
}

/// Only tight curves count: a gently round surface like a barrel is not an edge. Lifting it would put
/// it on a wear mask's threshold, and the mask's noise would show as patches.
fn tight(v: f32) -> f32 {
    v.signum() * (v.abs() - CURVATURE_GENTLE).max(0.0)
}

/// What shading and faces agree on. A flat low poly face only turns in its shading, a smooth shaded
/// low poly corner only in its faces; a real rounded surface turns in both.
fn agreed(shading: f32, faces: f32) -> f32 {
    if shading * faces <= 0.0 { 0.0 } else { shading.abs().min(faces.abs()).copysign(shading) }
}

impl Curvature {
    fn new(s: &BakeSettings, low: &Mesh, diag: f32, tiles: (u32, u32)) -> Curvature {
        let on = s.maps.contains(&BakedMap::Curvature) && !s.curvature_from_normal && s.curvature_strength != 0.0;
        // UVs are in atlas space: across the whole tile grid, each tile `resolution` texels wide.
        let resolution = s.resolution.max(1) as f32 * ((tiles.0.max(1) * tiles.1.max(1)) as f32).sqrt();
        let texel = |tri: usize| {
            let [a, b, c] = low.tri(tri)?;
            let (pa, pb, pc) = (*low.positions.get(a)?, *low.positions.get(b)?, *low.positions.get(c)?);
            let (ua, ub, uc) = (*low.uvs.get(a)?, *low.uvs.get(b)?, *low.uvs.get(c)?);
            let world = (pb - pa).cross(pc - pa).length();
            let uv = (ub - ua).perp_dot(uc - ua).abs();
            (uv > 1e-12 && world > 0.0).then(|| (world / uv).sqrt() / resolution)
        };
        let fallback = diag * 1e-3;
        let tris = if on { low.triangle_count() } else { 0 };
        let walls = CURVATURE_WALLS;
        Curvature {
            on,
            strength: s.curvature_strength,
            walls,
            spread: walls * 4.0 / 3.0,
            fallback,
            texels: (0..tris).into_par_iter().map(|t| texel(t).filter(|v| v.is_finite()).unwrap_or(fallback)).collect(),
            low: Tuning { convex: CURVATURE_CONVEX, true_radius: diag * CURVATURE_TRUE_RADIUS, true_strength: CURVATURE_TRUE },
            high: Tuning { convex: CURVATURE_CONVEX_HIGH, true_radius: diag * CURVATURE_TRUE_RADIUS_HIGH, true_strength: CURVATURE_TRUE_HIGH },
            ring: std::array::from_fn(|i| {
                let a = std::f32::consts::TAU * i as f32 / CURVATURE_RAYS as f32;
                (a.cos(), a.sin())
            }),
        }
    }

    /// World size of one texel on low poly triangle `tri`.
    fn texel(&self, tri: u32) -> f32 {
        self.texels.get(tri as usize).copied().unwrap_or(self.fallback)
    }

    /// The rings' radii around a sample whose low poly texel is `texel`.
    fn radii(&self, tuning: &Tuning, texel: f32) -> [f32; CURVATURE_RINGS] {
        let r = self.walls * texel;
        [r, r, self.spread * texel, tuning.true_radius]
    }

    /// The directions of a ring around a surface.
    fn directions(&self, t: Vec3, b: Vec3) -> [Vec3; CURVATURE_RAYS] {
        self.ring.map(|(cos, sin)| t * cos + b * sin)
    }

    /// Every ring's rays of one sample, in order, hitting only triangles of `key`.
    fn rays(&self, tuning: &Tuning, surf: &Surface, texel: f32, key: u32, out: &mut [Ray]) {
        let (geo, t, b) = surface_frame(surf);
        let directions = self.directions(t, b);
        let radii = self.radii(tuning, texel);
        for (ring, (rays, r)) in out.chunks_exact_mut(CURVATURE_RAYS).zip(radii).enumerate() {
            for (ray, d) in rays.iter_mut().zip(directions) {
                *ray = match ring {
                    0 => Ray { origin: surf.pos + geo * (0.15 * r), dir: d, tmax: r, key },
                    1 => Ray { origin: surf.pos - geo * (0.15 * r), dir: d, tmax: r, key },
                    _ => Ray { origin: surf.pos + d * r + geo * (0.25 * r), dir: -geo, tmax: 2.25 * r, key },
                };
            }
        }
    }

    /// The check of each wall above: a contact stands on this surface, so just before the wall the
    /// ground is still there. `None` for a ray that hit no wall.
    fn ground_rays(&self, surf: &Surface, texel: f32, key: u32, walls: &[Option<Hit>], out: &mut Vec<Ray>) {
        let (geo, t, b) = surface_frame(surf);
        let r = self.walls * texel;
        for (w, d) in walls.iter().zip(self.directions(t, b)) {
            if let Some(h) = w {
                out.push(Ray { origin: surf.pos + geo * (0.15 * r) + d * (h.t * 0.85), dir: -geo, tmax: 0.5 * r, key });
            }
        }
    }

    /// How much wall stands around the sample. A hit counts by how upright the surface hit is and by
    /// how far the shading normal really breaks there: a smooth shaded corner is a smooth surface,
    /// however low poly. `keep`: whether each hit counts at all.
    fn wall(&self, src: &Source, surf: &Surface, geo: Vec3, r: f32, hits: &[Option<Hit>], keep: &mut dyn FnMut() -> bool) -> f32 {
        let mut sum = 0.0;
        for h in hits.iter().flatten() {
            if !keep() {
                continue;
            }
            let upright = triangle_normal(src.mesh, h.tri).map_or(1.0, |n| (1.0 - n.dot(geo).powi(2)).max(0.0).sqrt());
            let shaded = interpolate(src.mesh, &src.mesh.normals, h.tri, [h.u, h.v]).and_then(Vec3::try_normalize).unwrap_or(surf.nrm);
            let hard = ((1.0 - shaded.dot(surf.nrm)) / CURVATURE_HARD_EDGE).clamp(0.0, 1.0);
            sum += (1.0 - h.t / r) * upright * hard;
        }
        sum / CURVATURE_RAYS as f32
    }

    /// How the normals spread apart (positive, convex) or lean together around the sample, as the
    /// mean turn along the ring: of the smooth shading, and of the faces. `turn`: the most the shading
    /// may turn (1 − cos) for a sample to count.
    fn spread(&self, src: &Source, surf: &Surface, geo: Vec3, directions: &[Vec3; CURVATURE_RAYS], hits: &[Option<Hit>], turn: f32) -> (f32, f32) {
        let (mut shading, mut faces) = (0.0, 0.0);
        for (h, dir) in hits.iter().zip(directions) {
            let Some(h) = h else { continue };
            let n = interpolate(src.mesh, &src.mesh.normals, h.tri, [h.u, h.v]).and_then(Vec3::try_normalize).unwrap_or(surf.nrm);
            if 1.0 - n.dot(surf.nrm) <= turn {
                shading += (n - surf.nrm).dot(*dir);
                if let Some(f) = triangle_normal(src.mesh, h.tri) {
                    let f = if f.dot(geo) < 0.0 { -f } else { f };
                    faces += (f - geo).dot(*dir);
                }
            }
        }
        (shading / CURVATURE_RAYS as f32, faces / CURVATURE_RAYS as f32)
    }

    /// The value of one sample from its rings' hits, and whether each wall above stands on the ground.
    fn value(&self, tuning: &Tuning, src: &Source, surf: &Surface, texel: f32, hits: &[Option<Hit>], grounds: &[Option<Hit>]) -> f32 {
        let (geo, t, b) = surface_frame(surf);
        let directions = self.directions(t, b);
        let ring = |n: usize| hits.get(n * CURVATURE_RAYS..(n + 1) * CURVATURE_RAYS).unwrap_or_default();
        let r = self.walls * texel;
        let mut next = grounds.iter();
        let above = self.wall(src, surf, geo, r, ring(0), &mut || next.next().copied().flatten().is_some_and(|g| g.t < 0.3 * r));
        let below = self.wall(src, surf, geo, r, ring(1), &mut || true);
        let (shading, _) = self.spread(src, surf, geo, &directions, ring(2), 2.0);
        let (true_shading, true_faces) = self.spread(src, surf, geo, &directions, ring(3), 2.0);
        let v = -above * CURVATURE_CONCAVE
            + (below - above).max(0.0) * tuning.convex
            + shading * 0.5
            + tight(agreed(true_shading, true_faces) * tuning.true_strength);
        (0.5 + v * self.strength).clamp(0.0, 1.0)
    }
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
    pub curvature: f32,
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
    curvature: Curvature,
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
        let low = Source::new(req.mesh, req.bvh, &[], &s.low_suffix, &mut keys);
        let high = req.high.map(|h| Source::new(h.mesh, h.bvh, h.base_color, &s.high_suffix, &mut keys));
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
            curvature: Curvature::new(s, req.mesh, diag, req.tiles),
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
        self.has_high() || self.curvature.on || self.ao.is_some() || self.bent.is_some() || self.thickness.is_some()
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

    /// Whether this bake traces curvature rays.
    pub(crate) fn traces_curvature(&self) -> bool {
        self.curvature.on
    }

    /// Curvature of every sample: `(low poly surface, transfer)`. Rays go in batches per mesh, the
    /// ground checks in a second pass.
    pub(crate) fn trace_curvature(&self, scene: &mut dyn RayScene, samples: &[(&Surface, Option<&Transfer>)]) -> Result<Vec<f32>, BakeError> {
        let mut out = vec![0.5; samples.len()];
        let c = &self.curvature;
        for mesh in [0usize, 1] {
            let Some(src) = (if mesh == 0 { Some(&self.low) } else { self.high.as_ref() }) else { continue };
            let tuning = if mesh == 0 { &c.low } else { &c.high };
            // Matched by name, a high poly part only reads itself, like its transfer rays.
            let key = |surf: &Surface| if mesh == 1 && self.by_name { src.key.get(surf.tri as usize).copied().unwrap_or(ANY) } else { ANY };
            let chosen: Vec<(usize, &Surface, f32)> = samples
                .par_iter()
                .enumerate()
                .filter_map(|(k, (s, tr))| {
                    let surf = match (mesh, tr) {
                        (1, Some(t)) => &t.surf,
                        (0, None) => *s,
                        _ => return None,
                    };
                    Some((k, surf, c.texel(s.tri)))
                })
                .collect();
            let per = CURVATURE_RINGS * CURVATURE_RAYS;
            // A batch can hold whole rows of a big supersampled map: cast them in chunks.
            for chunk in chosen.chunks(CURVATURE_CHUNK) {
                let mut rays: Vec<Ray> = bytemuck::zeroed_vec(chunk.len() * per);
                rays.par_chunks_mut(per).zip(chunk).for_each(|(out, (_, surf, texel))| c.rays(tuning, surf, *texel, key(surf), out));
                let hits = scene.closest(mesh, &rays)?;
                let walls = |i: usize| hits.get(i * per..i * per + CURVATURE_RAYS).unwrap_or_default();
                let grounds: Vec<Ray> = chunk
                    .par_iter()
                    .enumerate()
                    .flat_map_iter(|(i, (_, surf, texel))| {
                        let mut v = Vec::new();
                        c.ground_rays(surf, *texel, key(surf), walls(i), &mut v);
                        v
                    })
                    .collect();
                let ground_hits = scene.closest(mesh, &grounds)?;
                // Each sample's share of the ground checks.
                let firsts: Vec<usize> = (0..chunk.len())
                    .scan(0, |first, i| {
                        let at = *first;
                        *first += walls(i).iter().flatten().count();
                        Some(at)
                    })
                    .collect();
                let values: Vec<(usize, f32)> = chunk
                    .par_iter()
                    .zip(&firsts)
                    .enumerate()
                    .map(|(i, ((k, surf, texel), first))| {
                        let n = walls(i).iter().flatten().count();
                        let g = ground_hits.get(*first..*first + n).unwrap_or_default();
                        let h = hits.get(i * per..(i + 1) * per).unwrap_or_default();
                        (*k, c.value(tuning, src, surf, *texel, h, g))
                    })
                    .collect();
                for (k, v) in values {
                    if let Some(o) = out.get_mut(k) {
                        *o = v;
                    }
                }
            }
        }
        Ok(out)
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
                BakedMap::Curvature => [traced.curvature; 3],
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
