//! Conversion of composited tiles to the texture formats the viewport samples.

use texturecraft_core::{BakedMap, Channel};
use texturecraft_texel::{TILE, TILE_AREA, srgb_lut};

use crate::eval::Composite;
use crate::store::BakedMaps;

/// One of the textures the viewport shader samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DisplayKind {
    /// sRGB8: base color, alpha = opacity.
    BaseColor,
    /// RGBA8 linear: ambient occlusion (channel × baked), roughness (1 − glossiness when the set
    /// has a Glossiness channel), metallic, specular level.
    Orm,
    /// RGBA8: painted tangent-space normal.
    Normal,
    /// RGBA16F: height in r, the linear Specular (F0) color in gba.
    Height,
    /// sRGB8: emissive color.
    Emissive,
}

impl DisplayKind {
    pub const ALL: [DisplayKind; 5] = [DisplayKind::BaseColor, DisplayKind::Orm, DisplayKind::Normal, DisplayKind::Height, DisplayKind::Emissive];

    /// Bytes per texel of the upload format.
    pub fn bytes_per_texel(self) -> usize {
        if self == DisplayKind::Height { 8 } else { 4 }
    }

    /// Display textures affected by a channel.
    pub fn of_channel(c: Channel) -> DisplayKind {
        match c {
            Channel::BaseColor | Channel::Opacity => DisplayKind::BaseColor,
            Channel::Normal => DisplayKind::Normal,
            Channel::Height | Channel::Specular => DisplayKind::Height,
            Channel::Emissive => DisplayKind::Emissive,
            _ => DisplayKind::Orm,
        }
    }
}

fn u8_of(v: u16) -> u8 {
    (v >> 8) as u8
}

/// Tile bytes (`TILE × TILE`, row-major) for `kind` from the composite, or None when the tile is
/// absent in every source (the GPU texture keeps its cleared default).
pub fn tile_bytes(comp: &Composite, baked: &BakedMaps, kind: DisplayKind, tx: u32, ty: u32) -> Vec<u8> {
    let bpt = kind.bytes_per_texel();
    let mut out = vec![0u8; TILE_AREA * bpt];
    let lut = srgb_lut();
    let get = |c: Channel| comp.channels.get(&c).and_then(|img| img.tile(tx, ty));
    let res = comp.resolution;
    let default16 = |c: Channel| texturecraft_texel::to_u16(c.default_value()[0]);
    match kind {
        DisplayKind::BaseColor | DisplayKind::Emissive => {
            let (c, alpha) = if kind == DisplayKind::BaseColor { (Channel::BaseColor, get(Channel::Opacity)) } else { (Channel::Emissive, None) };
            let src = get(c);
            let d = c.default_value().map(|v| (texturecraft_texel::linear_to_srgb(v) * 255.0 + 0.5) as u8);
            for i in 0..TILE_AREA {
                let px = &mut out[i * 4..i * 4 + 4];
                match src {
                    Some(s) => {
                        for k in 0..3 {
                            px[k] = lut[(s.get(i * 3 + k).copied().unwrap_or(0) >> 4) as usize];
                        }
                    }
                    None => px[..3].copy_from_slice(&d),
                }
                px[3] = alpha.map(|a| u8_of(a.get(i).copied().unwrap_or(65535))).unwrap_or(255);
            }
        }
        DisplayKind::Orm => {
            let (ao, m, s) = (get(Channel::AmbientOcclusion), get(Channel::Metallic), get(Channel::SpecularLevel));
            // Specular/glossiness sets: the viewport shades roughness = 1 − glossiness.
            let gloss = comp.channels.contains_key(&Channel::Glossiness);
            let (r, r_default) = if gloss { (get(Channel::Glossiness), default16(Channel::Glossiness)) } else { (get(Channel::Roughness), default16(Channel::Roughness)) };
            for i in 0..TILE_AREA {
                let (x, y) = (tx * TILE + (i as u32 % TILE), ty * TILE + (i as u32 / TILE));
                let baked_ao = baked.scalar(BakedMap::AmbientOcclusion, x, y, res).unwrap_or(1.0);
                let ch_ao = ao.and_then(|a| a.get(i)).map(|v| *v as f32 / 65535.0).unwrap_or(1.0);
                let px = &mut out[i * 4..i * 4 + 4];
                px[0] = ((baked_ao * ch_ao).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                let v = r.and_then(|t| t.get(i).copied()).unwrap_or(r_default);
                px[1] = u8_of(if gloss { 65535 - v } else { v });
                px[2] = u8_of(m.and_then(|t| t.get(i).copied()).unwrap_or(0));
                px[3] = u8_of(s.and_then(|t| t.get(i).copied()).unwrap_or(default16(Channel::SpecularLevel)));
            }
        }
        DisplayKind::Normal => {
            let n = get(Channel::Normal);
            for i in 0..TILE_AREA {
                let px = &mut out[i * 4..i * 4 + 4];
                match n {
                    Some(t) => {
                        for (k, d) in px.iter_mut().take(3).enumerate() {
                            *d = u8_of(t.get(i * 3 + k).copied().unwrap_or(0));
                        }
                    }
                    None => px[..3].copy_from_slice(&[128, 128, 255]),
                }
                px[3] = 255;
            }
        }
        DisplayKind::Height => {
            let (h, spec) = (get(Channel::Height), get(Channel::Specular));
            let spec_default = Channel::Specular.default_value();
            for i in 0..TILE_AREA {
                let v = h.and_then(|t| t.get(i)).map(|v| texturecraft_texel::from_u16_signed(*v)).unwrap_or(0.0);
                let f0 = |k: usize| spec.and_then(|t| t.get(i * 3 + k)).map(|v| *v as f32 / 65535.0).unwrap_or(spec_default[k]);
                for (k, c) in [v, f0(0), f0(1), f0(2)].into_iter().enumerate() {
                    out[i * 8 + k * 2..i * 8 + k * 2 + 2].copy_from_slice(&half::f16::from_f32(c).to_le_bytes());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use texturecraft_texel::to_u16;

    #[test]
    fn base_color_uses_srgb_and_opacity() {
        let mut comp = Composite::new(&[Channel::BaseColor, Channel::Opacity], 64);
        comp.channels.get_mut(&Channel::BaseColor).unwrap().set(0, 0, &[to_u16(1.0), to_u16(0.5), 0]);
        comp.channels.get_mut(&Channel::Opacity).unwrap().set(0, 0, &[to_u16(0.5)]);
        let b = tile_bytes(&comp, &BakedMaps::default(), DisplayKind::BaseColor, 0, 0);
        assert_eq!(b[0], 255);
        assert!((b[1] as i32 - 188).abs() <= 1, "{}", b[1]);
        assert_eq!(b[2], 0);
        assert!((b[3] as i32 - 127).abs() <= 1);
    }

    #[test]
    fn missing_channels_fall_back_to_defaults() {
        let comp = Composite::new(&[], 64);
        let orm = tile_bytes(&comp, &BakedMaps::default(), DisplayKind::Orm, 0, 0);
        assert_eq!(&orm[..4], &[255, 128, 0, 128]);
        let n = tile_bytes(&comp, &BakedMaps::default(), DisplayKind::Normal, 0, 0);
        assert_eq!(&n[..4], &[128, 128, 255, 255]);
        let h = tile_bytes(&comp, &BakedMaps::default(), DisplayKind::Height, 0, 0);
        assert_eq!(h.len(), TILE_AREA * 8);
        assert_eq!(half::f16::from_le_bytes([h[2], h[3]]).to_f32(), half::f16::from_f32(0.04).to_f32(), "default F0");
    }

    #[test]
    fn glossiness_shows_as_roughness_and_specular_rides_in_height() {
        let mut comp = Composite::new(&[Channel::Glossiness, Channel::Specular], 64);
        comp.channels.get_mut(&Channel::Glossiness).unwrap().set(0, 0, &[to_u16(0.8)]);
        comp.channels.get_mut(&Channel::Specular).unwrap().set(0, 0, &[to_u16(1.0), to_u16(0.5), 0]);
        let orm = tile_bytes(&comp, &BakedMaps::default(), DisplayKind::Orm, 0, 0);
        assert!((orm[1] as i32 - 51).abs() <= 1, "roughness = 1 - 0.8, got {}", orm[1]);
        let h = tile_bytes(&comp, &BakedMaps::default(), DisplayKind::Height, 0, 0);
        let at = |k: usize| half::f16::from_le_bytes([h[k * 2], h[k * 2 + 1]]).to_f32();
        assert_eq!(at(0), 0.0);
        assert!((at(1) - 1.0).abs() < 1e-3 && (at(2) - 0.5).abs() < 1e-3 && at(3) == 0.0);
        assert_eq!(DisplayKind::of_channel(Channel::Specular), DisplayKind::Height);
        assert_eq!(DisplayKind::of_channel(Channel::Glossiness), DisplayKind::Orm);
    }
}
