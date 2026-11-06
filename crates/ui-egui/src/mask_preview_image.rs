//! Shading of the smart mask previews: the mask as white over a near-black surface.

use egui::{Color32, ColorImage};
use texturecraft_engine::geom::glam::Vec3;
use texturecraft_engine::mask_preview::MaskPreviewPixel;
use texturecraft_engine::texel::linear_to_srgb;

/// View-space directions of the key light (upper left) and a weaker fill (right), and how much
/// each lights the surface.
const KEY: [f32; 3] = [-0.72, 0.62, 0.3];
const FILL: [f32; 3] = [0.7, -0.1, 0.7];
const AMBIENT: f32 = 0.1;
const KEY_STRENGTH: f32 = 0.8;
const FILL_STRENGTH: f32 = 0.35;
/// Where the mask is empty: a near-black albedo with a faint sheen of the key light, so the
/// shape reads without looking glossy.
const UNMASKED_ALBEDO: f32 = 0.006;
const SHEEN: f32 = 0.1;
const SHEEN_EXPONENT: f32 = 40.0;
/// Where the mask is full: white, darkened at most this much where unlit.
const WHITE_SHADING: f32 = 0.2;

/// A smart mask preview (sRGB, transparent outside the shape) from preview pixels (row-major
/// `size²`): the mask as white over a near-black surface lit softly from the upper left. No
/// environment reflections.
pub fn mask_preview_image(size: usize, pixels: &[Option<MaskPreviewPixel>]) -> ColorImage {
    let key = Vec3::from(KEY).normalize();
    let fill = Vec3::from(FILL).normalize();
    let half = (key + Vec3::Z).normalize();
    let mut img = ColorImage::filled([size, size], Color32::TRANSPARENT);

    for (out, p) in img.pixels.iter_mut().zip(pixels) {
        let Some(p) = p else { continue };

        let n = p.normal;
        let shading = AMBIENT + KEY_STRENGTH * n.dot(key).max(0.0) + FILL_STRENGTH * n.dot(fill).max(0.0);
        let unmasked = UNMASKED_ALBEDO * shading + SHEEN * n.dot(half).max(0.0).powf(SHEEN_EXPONENT);
        let masked = 1.0 - WHITE_SHADING * (1.0 - shading.min(1.0));
        let linear = unmasked + (masked - unmasked) * p.mask.clamp(0.0, 1.0);
        let gray = (linear_to_srgb(linear.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8;
        *out = Color32::from_rgba_unmultiplied(gray, gray, gray, (p.coverage.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
    }
    img
}
