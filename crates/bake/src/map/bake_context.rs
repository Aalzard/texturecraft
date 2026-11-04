use super::curvature::{Curvature, RAYS_PER_RING, RING_COUNT};
use super::sampling::{interpolate, material_color, pack, splitmix64, tangent_normal, welded};
use super::{source::Source, traced::Traced, transfer::Transfer};
use crate::skew::PreparedSkew;
use crate::tracing::{ANY, Hemisphere, Query, Ray, RayKind, RayScene, TraceMesh};
use crate::{BakeError, BakeRequest};
use rayon::prelude::*;
use std::collections::HashMap;
use texturecraft_core::{BackfaceMode, BakedMap, CurvatureMethod, PositionMode, RayDistribution, RaySettings, SelfOcclusion};
use texturecraft_geom::glam::Vec3;
use texturecraft_geom::{Hit, Mesh, Surface, surface_at};

const CURVATURE_CHUNK: usize = 1 << 15;

pub(crate) struct BakeContext<'a> {
    low: Source<'a>,
    high: Option<Source<'a>>,
    cage: Option<&'a Mesh>,
    skew: Option<PreparedSkew<'a>>,
    ray_normals: Vec<Vec3>,
    lo: Vec3,
    extent: Vec3,
    eps: f32,
    frontal: f32,
    rear: f32,
    position_mode: PositionMode,
    curvature_method: CurvatureMethod,
    curvature: Curvature,
    pub ao: Option<Hemisphere>,
    pub bent: Option<Hemisphere>,
    pub thickness: Option<Hemisphere>,
}

impl<'a> BakeContext<'a> {
    pub(crate) fn new(request: &BakeRequest<'a>) -> Result<BakeContext<'a>, BakeError> {
        let settings = request.settings;
        let (bounds_min, bounds_max) = request.mesh.bounds();
        let mesh_diagonal = (bounds_max - bounds_min).length().max(1e-6);
        if !mesh_diagonal.is_finite() {
            return Err(BakeError::Invalid("mesh bounds are too large".into()));
        }

        let mut name_keys = HashMap::new();
        let low = Source::new(request.mesh, request.bvh, &[], &settings.high_poly.low_suffix, &mut name_keys);
        let high =
            request.high.map(|high_mesh| Source::new(high_mesh.mesh, high_mesh.bvh, high_mesh.base_color, &settings.high_poly.high_suffix, &mut name_keys));
        let ray_normals = match settings.cage.average_normals && high.is_some() {
            true => {
                let (welded_indices, _, normals) = welded(request.mesh, mesh_diagonal);
                welded_indices.iter().map(|index| normals.get(*index as usize).copied().unwrap_or(Vec3::Y)).collect()
            }
            false => Vec::new(),
        };

        let skew = request.skew.map(|skew_map| PreparedSkew::new(skew_map, request.mesh, mesh_diagonal)).transpose()?;
        let ray_epsilon = 1e-4 * mesh_diagonal;
        let pass = |map: BakedMap, ray_settings: &RaySettings, kind: RayKind| {
            let half_angle_radians = (ray_settings.spread.clamp(0.0, 180.0) * 0.5).to_radians();
            let occlusion = kind == RayKind::Occlusion;
            settings.maps.is_enabled(map).then_some(Hemisphere {
                kind,
                rays: ray_settings.rays,
                dmax: ray_settings.max_distance * mesh_diagonal,
                eps: ray_epsilon,
                cos_max: half_angle_radians.cos(),
                sin_max: half_angle_radians.sin(),
                uniform: ray_settings.distribution == RayDistribution::Uniform,
                ignore_backface: occlusion && settings.maps.ao.parameters.ignore_backface != BackfaceMode::Never,
                same_key: occlusion && settings.maps.ao.parameters.self_occlusion == SelfOcclusion::SameMeshName,
            })
        };

        Ok(BakeContext {
            low,
            high,
            cage: request.cage,
            skew,
            ray_normals,
            lo: bounds_min,
            extent: bounds_max - bounds_min,
            eps: ray_epsilon,
            frontal: settings.cage.max_frontal * mesh_diagonal,
            rear: settings.cage.max_rear * mesh_diagonal,
            position_mode: settings.maps.position.parameters.normalization,
            curvature_method: settings.maps.curvature.parameters.method,
            curvature: Curvature::new(settings, request.mesh, mesh_diagonal, request.tiles),
            ao: pass(BakedMap::AmbientOcclusion, &settings.maps.ao.parameters.rays, RayKind::Occlusion),
            bent: pass(BakedMap::BentNormal, &settings.maps.bent_normal.parameters.rays, RayKind::Occlusion),
            thickness: pass(BakedMap::Thickness, &settings.maps.thickness.parameters.rays, RayKind::Thickness),
        })
    }

    pub(crate) fn has_high(&self) -> bool {
        self.high.is_some()
    }

    pub(crate) fn casts_rays(&self) -> bool {
        self.has_high() || self.curvature.is_enabled || self.ao.is_some() || self.bent.is_some() || self.thickness.is_some()
    }

    pub(crate) fn trace_meshes(&self) -> Vec<TraceMesh<'_>> {
        std::iter::once(&self.low).chain(&self.high).map(Source::trace).collect()
    }

    pub(crate) fn transfer_ray(&self, surface: &Surface, barycentric: [f32; 2]) -> (Ray, f32) {
        let averaged_normal = interpolate(self.low.mesh, &self.ray_normals, surface.tri, barycentric).and_then(Vec3::try_normalize).unwrap_or(surface.nrm);
        let cage = self
            .cage
            .and_then(|cage_mesh| interpolate(cage_mesh, &cage_mesh.positions, surface.tri, barycentric))
            .map(|cage_position| surface.pos - cage_position)
            .filter(|offset| offset.length() > self.eps);
        let (origin, direction, front) = match cage {
            Some(offset) => (surface.pos - offset, offset.normalize(), offset.length()),
            None => {
                let up = match &self.skew {
                    Some(skew) => skew.correct(self.low.mesh, surface, averaged_normal),
                    None => averaged_normal,
                };
                (surface.pos + up * self.frontal, -up, self.frontal)
            }
        };
        (Ray { origin, direction, max_distance: front + self.rear, key: ANY }, front)
    }

    pub(crate) fn transfer(&self, hit: Option<Hit>, front: f32) -> Option<Transfer> {
        let (high, hit) = (self.high.as_ref()?, hit?);
        let surface = surface_at(high.mesh, hit.tri, hit.u, hit.v)?;
        Some(Transfer { surface, bary: [hit.u, hit.v], t: hit.t, front })
    }

    pub(crate) fn query(&self, transfer: Option<&Transfer>, low_surface: &Surface, seed: u64) -> Query {
        let (source, surface, mesh) = match (&self.high, transfer) {
            (Some(high), Some(transfer_hit)) => (high, &transfer_hit.surface, 1),
            _ => (&self.low, low_surface, 0),
        };
        let seed_hash = splitmix64(seed);
        let rot = [(seed_hash >> 40) as f32 / (1u64 << 24) as f32, ((seed_hash >> 16) & 0xff_ffff) as f32 / (1u64 << 24) as f32];
        let geo = if surface.geo_nrm.dot(surface.nrm) < 0.0 { -surface.geo_nrm } else { surface.geo_nrm };
        Query { pos: surface.pos, nrm: surface.nrm, geo, rot, key: source.key.get(surface.tri as usize).copied().unwrap_or(ANY), mesh }
    }

    pub(crate) fn traces_curvature(&self) -> bool {
        self.curvature.is_enabled
    }

    pub(crate) fn trace_curvature(&self, scene: &mut dyn RayScene, samples: &[(&Surface, Option<&Transfer>)]) -> Result<Vec<f32>, BakeError> {
        let mut output = vec![0.5; samples.len()];
        let curvature = &self.curvature;
        for mesh in [0usize, 1] {
            let Some(source) = (if mesh == 0 { Some(&self.low) } else { self.high.as_ref() }) else { continue };
            let tuning = if mesh == 0 { &curvature.low } else { &curvature.high };
            let chosen: Vec<(usize, &Surface, f32)> = samples
                .par_iter()
                .enumerate()
                .filter_map(|(sample_index, (low_surface, transfer_hit))| {
                    let surface = match (mesh, transfer_hit) {
                        (1, Some(hit)) => &hit.surface,
                        (0, None) => *low_surface,
                        _ => return None,
                    };
                    Some((sample_index, surface, curvature.texel(low_surface.tri)))
                })
                .collect();
            let rays_per_sample = RING_COUNT * RAYS_PER_RING;
            for chunk in chosen.chunks(CURVATURE_CHUNK) {
                let mut rays: Vec<Ray> = bytemuck::zeroed_vec(chunk.len() * rays_per_sample);
                rays.par_chunks_mut(rays_per_sample).zip(chunk).for_each(|(output, (_, surface, texel))| curvature.rays(tuning, surface, *texel, ANY, output));
                let hits = scene.closest(mesh, &rays)?;
                let walls = |chunk_index: usize| hits.get(chunk_index * rays_per_sample..chunk_index * rays_per_sample + RAYS_PER_RING).unwrap_or_default();
                let grounds: Vec<Ray> = chunk
                    .par_iter()
                    .enumerate()
                    .flat_map_iter(|(chunk_index, (_, surface, texel))| {
                        let mut value = Vec::new();
                        curvature.ground_rays(surface, *texel, ANY, walls(chunk_index), &mut value);
                        value
                    })
                    .collect();
                let ground_hits = scene.closest(mesh, &grounds)?;
                let ground_offsets: Vec<usize> = (0..chunk.len())
                    .scan(0, |first, chunk_index| {
                        let offset = *first;
                        *first += walls(chunk_index).iter().flatten().count();
                        Some(offset)
                    })
                    .collect();
                let values: Vec<(usize, f32)> = chunk
                    .par_iter()
                    .zip(&ground_offsets)
                    .enumerate()
                    .map(|(chunk_index, ((sample_index, surface, texel), first))| {
                        let ground_count = walls(chunk_index).iter().flatten().count();
                        let sample_ground_hits = ground_hits.get(*first..*first + ground_count).unwrap_or_default();
                        let sample_hits = hits.get(chunk_index * rays_per_sample..(chunk_index + 1) * rays_per_sample).unwrap_or_default();
                        (*sample_index, curvature.value(tuning, source, surface, *texel, sample_hits, sample_ground_hits))
                    })
                    .collect();
                for (sample_index, value) in values {
                    if let Some(destination) = output.get_mut(sample_index) {
                        *destination = value;
                    }
                }
            }
        }
        Ok(output)
    }

    pub(crate) fn evaluate(
        &self,
        maps: &[BakedMap],
        low_surface: &Surface,
        barycentric: [f32; 2],
        transfer: Option<&Transfer>,
        traced: &Traced,
        outputs: &mut [[f32; 3]],
    ) {
        let (source, surface, source_barycentric) = match (&self.high, transfer) {
            (Some(high), Some(hit)) => (high, &hit.surface, hit.bary),
            _ => (&self.low, low_surface, barycentric),
        };

        let missed = self.high.is_some() && transfer.is_none();
        for (&map, output) in maps.iter().zip(outputs.iter_mut()) {
            *output = match map {
                BakedMap::AmbientOcclusion => [traced.ao; 3],
                BakedMap::BentNormal => pack(traced.bent.unwrap_or(surface.nrm)),
                BakedMap::Thickness => [traced.thickness; 3],
                BakedMap::Curvature if self.curvature_method == CurvatureMethod::NormalMap => [0.5; 3],
                BakedMap::Curvature => [traced.curvature; 3],
                BakedMap::WorldNormal => pack(surface.nrm),
                BakedMap::Position => self.position(surface.pos),
                BakedMap::Id => material_color(source.part.get(surface.tri as usize).copied().unwrap_or(0) as usize),
                BakedMap::Normal => transfer.map_or(map.neutral(), |hit| tangent_normal(low_surface, hit.surface.nrm)),
                BakedMap::Height => [transfer.map_or(0.5, |hit| (0.5 + (hit.front - hit.t) / (hit.front + self.rear)).clamp(0.0, 1.0)); 3],
                BakedMap::Opacity if missed => [0.0; 3],
                BakedMap::Opacity => [source.color(surface, source_barycentric).w; 3],
            };
        }
    }

    fn position(&self, position: Vec3) -> [f32; 3] {
        let normalized = match self.position_mode {
            PositionMode::BoundingBox => (position - self.lo) / self.extent.max(Vec3::splat(1e-20)),
            PositionMode::BoundingSphere => (position - (self.lo + self.extent * 0.5)) / self.extent.length().max(1e-20) + 0.5,
            PositionMode::None => return position.to_array(),
        };

        normalized.clamp(Vec3::ZERO, Vec3::ONE).to_array()
    }
}
