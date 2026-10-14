//! Pixel data owned by a document: painted layer channels, painted mask effects and baked mesh
//! maps. Everything is copy-on-write, so cloning the store for undo is cheap.

use std::collections::{BTreeMap, HashMap};

use texturecraft_core::{BakedMap, Channel, ChannelKind, LayerId};
use texturecraft_texel::{TileImage, from_u16, from_u16_signed};

/// Painted data of one paint layer channel: premultiplied value(s) plus coverage, u16 per
/// component (height stored signed).
#[derive(Clone, Debug, Default)]
pub struct PixelStore {
    /// (paint layer or content paint effect, channel) → texels.
    pub layers: HashMap<(LayerId, Channel), TileImage>,
    /// Paint effect of a mask → (premultiplied value, coverage).
    pub masks: HashMap<LayerId, TileImage>,
}

impl PixelStore {
    pub fn layer(&self, id: LayerId, c: Channel) -> Option<&TileImage> {
        self.layers.get(&(id, c))
    }

    /// The image for `(id, c)` at `res`, created empty if missing or of another size.
    pub fn layer_mut(&mut self, id: LayerId, c: Channel, res: u32) -> &mut TileImage {
        let img = self.layers.entry((id, c)).or_insert_with(|| TileImage::new(res, res, c.components() + 1));
        if img.width() != res {
            *img = TileImage::new(res, res, c.components() + 1);
        }
        img
    }

    pub fn mask_mut(&mut self, id: LayerId, res: u32) -> &mut TileImage {
        let img = self.masks.entry(id).or_insert_with(|| TileImage::new(res, res, 2));
        if img.width() != res {
            *img = TileImage::new(res, res, 2);
        }
        img
    }

    /// Drop pixels of layers that no longer exist.
    pub fn retain(&mut self, alive: impl Fn(LayerId) -> bool) {
        self.layers.retain(|(id, _), _| alive(*id));
        self.masks.retain(|id, _| alive(*id));
    }

    pub fn bytes(&self) -> usize {
        self.layers.values().chain(self.masks.values()).map(TileImage::bytes).sum()
    }
}

/// Decode a stored component of channel kind `kind` to its float value.
#[inline]
pub fn decode(kind: ChannelKind, v: u16) -> f32 {
    if kind == ChannelKind::Signed { from_u16_signed(v) } else { from_u16(v) }
}

/// Baked mesh maps of one texture set (dense, row-major, `components()` floats per texel).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BakedMaps {
    pub resolution: u32,
    pub maps: BTreeMap<BakedMap, std::sync::Arc<Vec<f32>>>,
}

impl BakedMaps {
    /// Sample map `m` at texel (x, y) of an image `res` wide (nearest texel of the bake).
    pub fn sample(&self, m: BakedMap, x: u32, y: u32, res: u32) -> Option<[f32; 3]> {
        let data = self.maps.get(&m)?;
        let r = self.resolution.max(1) as u64;
        let (bx, by) = ((x as u64 * r / res.max(1) as u64).min(r - 1), (y as u64 * r / res.max(1) as u64).min(r - 1));
        let comps = m.components();
        let i = ((by * r + bx) as usize) * comps;
        let s = data.get(i..i + comps)?;
        Some(if comps == 3 { [s[0], s[1], s[2]] } else { [s[0]; 3] })
    }

    pub fn scalar(&self, m: BakedMap, x: u32, y: u32, res: u32) -> Option<f32> {
        self.sample(m, x, y, res).map(|v| v[0])
    }
}

/// A decoded image resource sampled by fills (values in the file's encoding; see `srgb`).
#[derive(Clone, Debug, PartialEq)]
pub struct ImageRes {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<[f32; 4]>,
    /// 8/16-bit images are sRGB-encoded; color channels linearize them.
    pub srgb: bool,
}

/// Image resources by name.
pub type ImageStore = HashMap<String, std::sync::Arc<ImageRes>>;

impl ImageRes {
    fn texel(&self, x: i64, y: i64) -> [f32; 4] {
        let (w, h) = (self.width.max(1) as i64, self.height.max(1) as i64);
        let i = (y.rem_euclid(h) * w + x.rem_euclid(w)) as usize;
        self.rgba.get(i).copied().unwrap_or([0.0; 4])
    }

    /// Bilinear sample with wrap-around (texture coordinates, v down).
    pub fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        if !u.is_finite() || !v.is_finite() {
            return [0.0; 4];
        }
        let fx = u * self.width as f32 - 0.5;
        let fy = v * self.height as f32 - 0.5;
        let (x0, y0) = (fx.floor() as i64, fy.floor() as i64);
        let (tx, ty) = (fx - fx.floor(), fy - fy.floor());
        let (a, b, c, d) = (self.texel(x0, y0), self.texel(x0 + 1, y0), self.texel(x0, y0 + 1), self.texel(x0 + 1, y0 + 1));
        let mut out = [0.0; 4];
        for k in 0..4 {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bot = c[k] + (d[k] - c[k]) * tx;
            out[k] = top + (bot - top) * ty;
        }
        out
    }
}
