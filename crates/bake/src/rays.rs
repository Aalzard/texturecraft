//! The ray casts of a bake, in batches. [`CpuTracer`] is the oracle; a GPU [`Tracer`] (the
//! render crate's compute baker) must give the same results: it receives the same rays and
//! queries, and generates the same secondary-ray directions from each query's rotation.

use std::f32::consts::TAU;

use rayon::prelude::*;
use texturecraft_geom::glam::Vec3;
use texturecraft_geom::{Bvh, Hit, Mesh};

use crate::BakeError;

/// A ray or query key that matches every triangle (no part filter).
pub const ANY: u32 = u32::MAX;

/// A mesh rays are cast against: [`RayScene`] mesh 0 is the low poly, mesh 1 the high poly.
#[derive(Clone, Copy, Debug)]
pub struct TraceMesh<'a> {
    pub mesh: &'a Mesh,
    pub bvh: &'a Bvh,
    /// Part key of every triangle (original order); triangles of no part hold [`ANY`].
    pub keys: &'a [u32],
}

impl TraceMesh<'_> {
    fn key_matches(&self, tri: u32, key: u32) -> bool {
        key == ANY || self.keys.get(tri as usize) == Some(&key)
    }

    /// Whether `d` meets triangle `tri` from behind (against its winding's normal).
    fn back_facing(&self, tri: u32, d: Vec3) -> bool {
        let p = |i: usize| self.mesh.positions.get(i).copied().unwrap_or(Vec3::ZERO);
        self.mesh.tri(tri as usize).is_some_and(|[a, b, c]| (p(b) - p(a)).cross(p(c) - p(a)).dot(d) > 0.0)
    }
}

/// A closest-hit ray (the high-poly transfer).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
    pub tmax: f32,
    /// Only triangles with this part key are hit ([`ANY`]: all).
    pub key: u32,
}

/// One sample's secondary rays (ambient occlusion, bent normal or thickness).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Query {
    pub pos: Vec3,
    /// Shading normal: the axis of the ray cone.
    pub nrm: Vec3,
    /// Geometric normal on the shading normal's side: rays below its plane are skipped.
    pub geo: Vec3,
    /// Cranley–Patterson rotation of the sample's ray set.
    pub rot: [f32; 2],
    /// Part key of the sample's triangle (for [`Hemisphere::same_key`]).
    pub key: u32,
    /// Scene mesh the rays are cast against.
    pub mesh: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RayKind {
    /// Out of the surface: ambient occlusion and the bent normal.
    Occlusion,
    /// Into the surface: the distance through the mesh.
    Thickness,
}

/// Parameters of one pass of secondary rays (distances absolute).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hemisphere {
    pub kind: RayKind,
    pub rays: u32,
    pub dmax: f32,
    /// Ray start offset along the geometric normal.
    pub eps: f32,
    /// Cosine and sine of the cone's half angle.
    pub cos_max: f32,
    pub sin_max: f32,
    /// Uniform over the cone's solid angle (else cosine-weighted).
    pub uniform: bool,
    pub ignore_backface: bool,
    /// Only triangles with the query's part key occlude.
    pub same_key: bool,
}

impl Hemisphere {
    /// Direction `i` of a query's ray set around `axis` (basis `t`, `b`): Hammersley points
    /// rotated by `rot`, mapped onto the cone.
    pub fn direction(&self, (t, b, axis): (Vec3, Vec3, Vec3), i: u32, rot: [f32; 2]) -> Vec3 {
        let u1 = (i as f32 / self.rays as f32 + rot[0]).fract();
        let u2 = (i.reverse_bits() as f32 / 4_294_967_296.0 + rot[1]).fract();
        let (r, z) = if self.uniform {
            let z = 1.0 - u2 * (1.0 - self.cos_max);
            ((1.0 - z * z).max(0.0).sqrt(), z)
        } else {
            let r = self.sin_max * u2.sqrt();
            (r, (1.0 - r * r).max(0.0).sqrt())
        };
        let phi = TAU * u1;
        t * (r * phi.cos()) + b * (r * phi.sin()) + axis * z
    }

    /// The axis, geometric normal and ray origin of `q` for this kind of rays.
    fn frame(&self, q: &Query) -> (Vec3, Vec3, Vec3) {
        let (axis, geo) = match self.kind {
            RayKind::Occlusion => (q.nrm, q.geo),
            RayKind::Thickness => (-q.nrm, -q.geo),
        };
        (axis, geo, q.pos + geo * self.eps)
    }

    /// `q`'s result on `m`: occlusion gives (ambient occlusion, bent normal), thickness gives
    /// (thickness, 0, 0, 0); both relative and in 0..1 (1 = open / thick).
    pub fn evaluate(&self, m: &TraceMesh, q: &Query) -> [f32; 4] {
        let (axis, geo, o) = self.frame(q);
        let (t, b) = axis.any_orthonormal_pair();
        let key = if self.same_key && self.kind == RayKind::Occlusion { q.key } else { ANY };
        let filtered = key != ANY || (self.ignore_backface && self.kind == RayKind::Occlusion);
        let (mut sum, mut bent, mut n) = (0.0f32, Vec3::ZERO, 0u32);
        for i in 0..self.rays {
            let d = self.direction((t, b, axis), i, q.rot);
            if d.dot(geo) <= 0.0 {
                continue;
            }
            let hit = match filtered {
                true => m.bvh.closest_filtered(o, d, self.dmax, &|tri| m.key_matches(tri, key) && !(self.ignore_backface && m.back_facing(tri, d))),
                false => m.bvh.closest(o, d, self.dmax),
            };
            match self.kind {
                RayKind::Occlusion => {
                    let v = hit.map_or(0.0, |h| 1.0 - (h.t / self.dmax).powi(2));
                    sum += v;
                    bent += d * (1.0 - v);
                }
                RayKind::Thickness => sum += hit.map_or(self.dmax, |h| h.t),
            }
            n += 1;
        }
        match self.kind {
            RayKind::Occlusion => {
                let ao = if n == 0 { 1.0 } else { (1.0 - sum / n as f32).clamp(0.0, 1.0) };
                let bent = bent.try_normalize().unwrap_or(q.nrm);
                [ao, bent.x, bent.y, bent.z]
            }
            RayKind::Thickness => [if n == 0 { 1.0 } else { (sum / n as f32 / self.dmax).clamp(0.0, 1.0) }, 0.0, 0.0, 0.0],
        }
    }
}

/// Ray casts on the meshes of one bake.
pub trait RayScene: Send {
    /// The closest hit of every ray on scene mesh `mesh` (triangles of the ray's key).
    fn closest(&mut self, mesh: usize, rays: &[Ray]) -> Result<Vec<Option<Hit>>, BakeError>;
    /// [`Hemisphere::evaluate`] of every query, for each pass.
    fn hemispheres(&mut self, passes: &[Hemisphere], queries: &[Query]) -> Result<Vec<Vec<[f32; 4]>>, BakeError>;
}

/// Where ray casts run.
pub trait Tracer: Send + Sync {
    /// Shown in logs and command results ("CPU", the GPU's name).
    fn name(&self) -> String;
    /// Prepare `meshes` (low poly, then the high poly if any) for ray casts.
    fn scene<'a>(&'a self, meshes: &[TraceMesh<'a>]) -> Result<Box<dyn RayScene + 'a>, BakeError>;
}

/// Ray casts on the CPU (rayon over the batch).
#[derive(Clone, Copy, Debug, Default)]
pub struct CpuTracer;

struct CpuScene<'a>(Vec<TraceMesh<'a>>);

impl CpuScene<'_> {
    fn mesh(&self, i: usize) -> Result<&TraceMesh<'_>, BakeError> {
        self.0.get(i).ok_or_else(|| BakeError::Invalid(format!("no scene mesh {i}")))
    }
}

impl RayScene for CpuScene<'_> {
    fn closest(&mut self, mesh: usize, rays: &[Ray]) -> Result<Vec<Option<Hit>>, BakeError> {
        let m = self.mesh(mesh)?;
        Ok(rays
            .par_iter()
            .map(|r| match r.key {
                ANY => m.bvh.closest(r.origin, r.dir, r.tmax),
                key => m.bvh.closest_filtered(r.origin, r.dir, r.tmax, &|t| m.key_matches(t, key)),
            })
            .collect())
    }

    fn hemispheres(&mut self, passes: &[Hemisphere], queries: &[Query]) -> Result<Vec<Vec<[f32; 4]>>, BakeError> {
        passes.iter().map(|h| queries.par_iter().map(|q| Ok(h.evaluate(self.mesh(q.mesh as usize)?, q))).collect()).collect()
    }
}

impl Tracer for CpuTracer {
    fn name(&self) -> String {
        "CPU".into()
    }

    fn scene<'a>(&'a self, meshes: &[TraceMesh<'a>]) -> Result<Box<dyn RayScene + 'a>, BakeError> {
        Ok(Box::new(CpuScene(meshes.to_vec())))
    }
}
