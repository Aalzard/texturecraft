//! Smart mask shelf thumbnails: every smart mask on the baked preview shape, rendered together on
//! a worker the first time the shelf asks for one (baking the shape takes about two seconds on
//! the CPU; on the web the work runs inline).

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use egui::{Color32, ColorImage};
use texturecraft_engine::geom::glam::Vec3;
use texturecraft_engine::mask_preview::{MaskPreview, MaskPreviewPixel};
use texturecraft_engine::smart_masks::SMART_MASKS;
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
/// How often the shelf checks for the thumbnails while they render.
const POLL: Duration = Duration::from_millis(100);

/// Smart mask thumbnails by id, as the worker sends them.
type Thumbnails = Vec<(&'static str, ColorImage)>;

/// Where the smart mask thumbnails are.
#[derive(Default)]
pub enum MaskPreviews {
    #[default]
    NotStarted,
    Rendering(Receiver<Thumbnails>),
    /// Handed out (or the worker failed; the tiles keep their placeholder).
    Finished,
}

impl MaskPreviews {
    /// Start rendering `size²` thumbnails on the first call; the finished thumbnails on the call
    /// that finds them done (None before and after).
    pub fn poll(&mut self, ctx: &egui::Context, size: usize) -> Option<Thumbnails> {
        if matches!(self, MaskPreviews::NotStarted) {
            *self = MaskPreviews::Rendering(crate::spawn_worker("texturecraft-mask-previews", move || render(size)));
        }
        let MaskPreviews::Rendering(rx) = self else { return None };

        match rx.try_recv() {
            Ok(thumbnails) => {
                *self = MaskPreviews::Finished;
                Some(thumbnails)
            }
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(POLL);
                None
            }
            // The worker stopped without a result
            Err(TryRecvError::Disconnected) => {
                *self = MaskPreviews::Finished;
                None
            }
        }
    }
}

/// Every smart mask's `size²` thumbnail (empty when the preview shape fails to bake).
fn render(size: usize) -> Thumbnails {
    let mut preview = match MaskPreview::new(size) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("smart mask previews: {e}");
            return Vec::new();
        }
    };
    SMART_MASKS.iter().filter_map(|m| Some((m.id, image(size, &preview.render(m.id)?)))).collect()
}

/// A thumbnail (sRGB, transparent outside the shape) from preview pixels (row-major `size²`):
/// the mask as white over a near-black surface lit softly from the upper left. No environment
/// reflections.
fn image(size: usize, pixels: &[Option<MaskPreviewPixel>]) -> ColorImage {
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
