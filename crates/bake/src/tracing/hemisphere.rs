use super::{ANY, Query, RayKind, TraceMesh};
use std::f32::consts::TAU;
use texturecraft_geom::glam::Vec3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hemisphere {
    pub kind: RayKind,
    pub rays: u32,
    /// Maximum ray distance in mesh units
    pub dmax: f32,
    /// Ray origin offset along the geometric normal
    pub eps: f32,
    pub cos_max: f32,
    pub sin_max: f32,
    pub uniform: bool,
    pub ignore_backface: bool,
    pub same_key: bool,
}

impl Hemisphere {
    pub fn direction(&self, (tangent, bitangent, axis): (Vec3, Vec3, Vec3), ray_index: u32, rotation: [f32; 2]) -> Vec3 {
        let azimuth_sample = (ray_index as f32 / self.rays as f32 + rotation[0]).fract();
        let elevation_sample = (ray_index.reverse_bits() as f32 / 4_294_967_296.0 + rotation[1]).fract();
        let (radius, height) = if self.uniform {
            let height = 1.0 - elevation_sample * (1.0 - self.cos_max);
            ((1.0 - height * height).max(0.0).sqrt(), height)
        } else {
            let radius = self.sin_max * elevation_sample.sqrt();
            (radius, (1.0 - radius * radius).max(0.0).sqrt())
        };
        let azimuth = TAU * azimuth_sample;
        tangent * (radius * azimuth.cos()) + bitangent * (radius * azimuth.sin()) + axis * height
    }

    fn frame(&self, query: &Query) -> (Vec3, Vec3, Vec3) {
        let (axis, geometric_normal) = match self.kind {
            RayKind::Occlusion => (query.nrm, query.geo),
            RayKind::Thickness => (-query.nrm, -query.geo),
        };
        (axis, geometric_normal, query.pos + geometric_normal * self.eps)
    }

    pub fn evaluate(&self, mesh: &TraceMesh, query: &Query) -> [f32; 4] {
        let (axis, geometric_normal, origin) = self.frame(query);
        let (tangent, bitangent) = axis.any_orthonormal_pair();
        let key = if self.same_key && self.kind == RayKind::Occlusion { query.key } else { ANY };
        let filtered = key != ANY || (self.ignore_backface && self.kind == RayKind::Occlusion);
        let (mut accumulated, mut bent_normal, mut accepted_ray_count) = (0.0f32, Vec3::ZERO, 0u32);
        for ray_index in 0..self.rays {
            let direction = self.direction((tangent, bitangent, axis), ray_index, query.rot);
            if direction.dot(geometric_normal) <= 0.0 {
                continue;
            }
            let hit = match filtered {
                true => mesh.bvh.closest_filtered(origin, direction, self.dmax, &|tri| {
                    mesh.key_matches(tri, key) && !(self.ignore_backface && mesh.back_facing(tri, direction))
                }),
                false => mesh.bvh.closest(origin, direction, self.dmax),
            };
            match self.kind {
                RayKind::Occlusion => {
                    let occlusion = hit.map_or(0.0, |hit| 1.0 - (hit.t / self.dmax).powi(2));
                    accumulated += occlusion;
                    bent_normal += direction * (1.0 - occlusion);
                }
                RayKind::Thickness => accumulated += hit.map_or(self.dmax, |hit| hit.t),
            }
            accepted_ray_count += 1;
        }
        match self.kind {
            RayKind::Occlusion => {
                let ambient_occlusion = if accepted_ray_count == 0 { 1.0 } else { (1.0 - accumulated / accepted_ray_count as f32).clamp(0.0, 1.0) };
                let bent_normal = bent_normal.try_normalize().unwrap_or(query.nrm);
                [ambient_occlusion, bent_normal.x, bent_normal.y, bent_normal.z]
            }
            RayKind::Thickness => {
                [if accepted_ray_count == 0 { 1.0 } else { (accumulated / accepted_ray_count as f32 / self.dmax).clamp(0.0, 1.0) }, 0.0, 0.0, 0.0]
            }
        }
    }
}
