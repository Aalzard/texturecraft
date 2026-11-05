//! Smart mask shelf previews: the real smart masks on a baked preview shape. The shape
//! ([`primitives::preview_orb`]) is baked once; each preset is then applied with
//! `mask.smart.apply`, read back from the compositor and seen from a fixed camera.

use serde_json::json;
use texturecraft_core::{BakeResolution, BakedMap, CurvatureMethod, LayerId};
use texturecraft_geom::glam::{Quat, Vec2, Vec3};
use texturecraft_geom::{primitives, surface_at};

pub use crate::mask_preview_pixel::MaskPreviewPixel;
use crate::{EngineError, Result, Session};

/// Texture and bake size of the preview shape in texels (at 256 the edge lines break up).
const TEXTURE_PX: u32 = 512;
/// Camera rays per thumbnail pixel along each axis (anti-aliased outline).
const SUPERSAMPLE: usize = 2;
/// Bound camera samples and output allocations for this thumbnail-only renderer
const MAX_PREVIEW_SIZE_PX: usize = 512;
/// AO and thickness rays per texel (the default is 64; fewer look the same at thumbnail size).
const OCCLUSION_RAYS: u32 = 12;
/// Mesh curvature rays per texel (the default is 32; fewer look the same at thumbnail size).
const CURVATURE_RAYS: u32 = 12;
/// Mesh curvature sampling radius relative to the bounding box: wide enough that edges read as
/// lines at thumbnail size, narrow enough that the round body does not count as an edge.
const CURVATURE_RADIUS: f32 = 0.005;
/// The mesh maps the smart mask generators read.
const MAPS: [BakedMap; 3] = [BakedMap::Curvature, BakedMap::AmbientOcclusion, BakedMap::Thickness];
/// Camera: looking down by this angle, turned to show the front, left and top panels.
const CAMERA_PITCH_DEG: f32 = 35.0;
const CAMERA_YAW_DEG: f32 = -34.4;

/// What one camera sub-pixel sees: the index of its texel in the preview's texel list and the
/// view-space normal there (None where the ray misses).
type CameraSample = Option<(usize, Vec3)>;

/// The baked preview shape and what each thumbnail sub-pixel sees.
pub struct MaskPreview {
    session: Session,
    layer: LayerId,
    size: usize,
    /// The texels (x, y) the camera sees, each once.
    texels: Vec<(u32, u32)>,
    /// Per sub-pixel, row-major `(size · SUPERSAMPLE)²`.
    samples: Vec<CameraSample>,
}

impl MaskPreview {
    /// Bake the preview shape for square thumbnails with sides of 1..=512 pixels
    pub fn new(size: usize) -> Result<MaskPreview> {
        if !(1..=MAX_PREVIEW_SIZE_PX).contains(&size) {
            return Err(EngineError::Other(format!("mask preview size must be between 1 and {MAX_PREVIEW_SIZE_PX} pixels")));
        }

        let mut session = Session::new_project("Mask preview", primitives::preview_orb(), "builtin:preview_orb", None, TEXTURE_PX)?;
        let settings = &mut session.doc.project.texture_set_mut(0)?.bake;
        settings.common.resolution = BakeResolution::from_pixels(TEXTURE_PX).ok_or_else(|| EngineError::Other("invalid bake resolution".into()))?;
        settings.high_poly.use_low_as_high = true;
        settings.maps.curvature.parameters.method = CurvatureMethod::Mesh;
        settings.maps.curvature.parameters.mesh.sampling_radius = CURVATURE_RADIUS;
        settings.maps.curvature.parameters.mesh.rays = CURVATURE_RAYS;
        settings.maps.ao.parameters.rays.rays = OCCLUSION_RAYS;
        settings.maps.thickness.parameters.rays.rays = OCCLUSION_RAYS;

        for map in BakedMap::ALL {
            settings.maps.set_enabled(map, MAPS.contains(&map));
        }

        let layer = *session.doc.project.texture_set(0)?.stack.first().ok_or_else(|| EngineError::Other("the preview has no layer".into()))?;
        session.bake_now(0, false)?;

        let (texels, samples) = camera_samples(&session, size * SUPERSAMPLE);
        Ok(MaskPreview { session, layer, size, texels, samples })
    }

    /// Smart mask `id` per thumbnail pixel, row-major `size²`, None where the shape is not seen
    /// (None for an unknown id).
    pub fn render(&mut self, id: &str) -> Option<Vec<Option<MaskPreviewPixel>>> {
        crate::execute(&mut self.session, "mask.smart.apply", &json!({ "name": id, "layer": self.layer.0 })).ok()?;
        // Nothing is undone here; the snapshots would pile up over the presets
        self.session.undo.clear();
        let values = self.session.mask_at(self.layer, &self.texels)?;

        let fine = self.size * SUPERSAMPLE;
        let sub_pixels = (SUPERSAMPLE * SUPERSAMPLE) as f32;
        let pixels = (0..self.size * self.size)
            .map(|i| {
                let (x, y) = (i % self.size, i / self.size);
                let (mut mask, mut normal, mut hits) = (0.0, Vec3::ZERO, 0.0);
                for sy in 0..SUPERSAMPLE {
                    for sx in 0..SUPERSAMPLE {
                        let Some(&Some((texel, n))) = self.samples.get((y * SUPERSAMPLE + sy) * fine + x * SUPERSAMPLE + sx) else { continue };

                        mask += values.get(texel).copied().unwrap_or(0.0);
                        normal += n;
                        hits += 1.0;
                    }
                }
                (hits > 0.0).then(|| MaskPreviewPixel { mask: mask / hits, normal: normal.normalize_or_zero(), coverage: hits / sub_pixels })
            })
            .collect();
        Some(pixels)
    }
}

/// What each pixel of a `size²` orthographic view of the shape sees: the distinct texels hit, and
/// per pixel (row-major) its sample.
fn camera_samples(session: &Session, size: usize) -> (Vec<(u32, u32)>, Vec<CameraSample>) {
    let (lo, hi) = session.mesh.bounds();
    let center = (lo + hi) * 0.5;
    let radius = session.mesh.positions.iter().map(|p| p.distance(center)).fold(0.0f32, f32::max);
    let pitch = CAMERA_PITCH_DEG.to_radians();
    let turn = Quat::from_rotation_y(CAMERA_YAW_DEG.to_radians());
    let forward = turn * Vec3::new(0.0, -pitch.sin(), -pitch.cos());
    let right = turn * Vec3::X;
    let up = right.cross(forward);
    let max_texel = TEXTURE_PX - 1;

    let mut texels = Vec::new();
    let mut index_of = std::collections::HashMap::new();
    let samples = (0..size * size)
        .map(|i| {
            let ndc = Vec2::new(((i % size) as f32 + 0.5) / size as f32 * 2.0 - 1.0, 1.0 - ((i / size) as f32 + 0.5) / size as f32 * 2.0);
            // Start well outside the shape so every ray enters it from the front
            let origin = center + (right * ndc.x + up * ndc.y) * radius - forward * radius * 4.0;
            let hit = session.bvh.closest(origin, forward, radius * 8.0)?;
            let surface = surface_at(&session.mesh, hit.tri, hit.u, hit.v)?;
            let texel = (((surface.uv.x * TEXTURE_PX as f32) as u32).min(max_texel), ((surface.uv.y * TEXTURE_PX as f32) as u32).min(max_texel));
            let index = *index_of.entry(texel).or_insert_with(|| {
                texels.push(texel);
                texels.len() - 1
            });
            let n = surface.nrm;
            Some((index, Vec3::new(n.dot(right), n.dot(up), -n.dot(forward))))
        })
        .collect();
    (texels, samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mean absolute difference of the masks of two previews over the pixels both see.
    fn difference(a: &[Option<MaskPreviewPixel>], b: &[Option<MaskPreviewPixel>]) -> f32 {
        let pairs: Vec<(f32, f32)> = a.iter().zip(b).filter_map(|(a, b)| Some((a.as_ref()?.mask, b.as_ref()?.mask))).collect();
        pairs.iter().map(|(a, b)| (a - b).abs()).sum::<f32>() / pairs.len().max(1) as f32
    }

    #[test]
    fn invalid_preview_sizes_return_errors() {
        for size in [usize::MAX / 2 + 1, 0, MAX_PREVIEW_SIZE_PX + 1, usize::MAX] {
            assert!(MaskPreview::new(size).is_err(), "size {size} should be rejected before baking");
        }
    }

    #[test]
    fn smart_mask_previews_are_in_range_and_tell_edges_from_cavities() {
        let size = 24;
        let mut preview = MaskPreview::new(size).unwrap();
        for sm in crate::smart_masks::SMART_MASKS {
            let px = preview.render(sm.id).unwrap_or_else(|| panic!("{}", sm.id));
            assert_eq!(px.len(), size * size, "{}", sm.id);
            assert!(px[0].is_none(), "{}: the corners are outside the shape", sm.id);
            let seen: Vec<&MaskPreviewPixel> = px.iter().flatten().collect();
            assert!(seen.len() > size * size / 3, "{}: the shape fills the frame", sm.id);
            for p in seen {
                assert!((0.0..=1.0).contains(&p.mask) && (0.0..=1.0).contains(&p.coverage) && p.coverage > 0.0, "{}: {p:?}", sm.id);
                assert!(p.normal.is_finite(), "{}: {p:?}", sm.id);
            }
        }

        let (edges, cavities) = (preview.render("edges_sharp").unwrap(), preview.render("dirt_cavities").unwrap());
        let shows = |px: &[Option<MaskPreviewPixel>]| px.iter().flatten().any(|p| p.mask > 0.5);
        assert!(shows(&edges) && shows(&cavities), "both masks find something on the shape");
        assert!(difference(&edges, &cavities) > 0.05, "edges and cavities differ: {}", difference(&edges, &cavities));
        assert!(preview.render("nope").is_none());
    }
}
