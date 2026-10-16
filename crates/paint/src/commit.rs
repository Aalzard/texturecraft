//! Committing a stroke: premultiplied "over" (paint) or scale-down (erase) of the stroke alpha
//! into a layer's channels or a mask's paint effect. Only touched tiles are un-shared, so the
//! previous state stays intact in undo snapshots.

use std::collections::{BTreeMap, BTreeSet};

use rayon::prelude::*;
use texturecraft_compose::PixelStore;
use texturecraft_core::{Channel, ChannelKind, LayerId};
use texturecraft_texel::{TILE, TILE_AREA, TileImage, from_u16, from_u16_signed, to_u16, to_u16_signed};

pub(crate) fn dec(kind: ChannelKind, v: u16) -> f32 {
    if kind == ChannelKind::Signed { from_u16_signed(v) } else { from_u16(v) }
}

pub(crate) fn enc(kind: ChannelKind, v: f32) -> u16 {
    if kind == ChannelKind::Signed { to_u16_signed(v) } else { to_u16(v) }
}

/// Blend `alpha × opacity` into one image of `n` value components plus coverage. With a
/// per-texel source, its alpha factor scales the stroke and, when `stencil_color`, its color
/// replaces `value`; `carried` (clone, smudge) replaces `value` per texel.
fn over(img: &mut TileImage, kind: ChannelKind, value: &[f32], src: &CommitSource, stencil_color: bool, carried: Option<&TileImage>) {
    let comps = img.comps();
    let n = comps.saturating_sub(1);
    let opacity = src.opacity.clamp(0.0, 1.0);
    // New tiles start empty: zero coverage and a zero value (32768 in signed encoding).
    let mut empty = vec![enc(kind, 0.0); n];
    empty.push(0);
    let tiles: Vec<(u32, u32)> = src.tiles.iter().copied().collect();
    // Tiles blend independently: build each new tile in parallel, then swap them in. The old
    // tile data stays shared with undo snapshots.
    let image = &*img;
    let updated: Vec<((u32, u32), Vec<u16>)> = tiles
        .par_iter()
        .filter_map(|&(tx, ty)| {
            let a = src.alpha.tile(tx, ty)?;
            if src.erase && !image.is_allocated(tx, ty) {
                return None;
            }
            let carried = match carried.map(|c| c.tile(tx, ty)) {
                Some(None) => return None,
                Some(Some(t)) => Some(t),
                None => None,
            };
            let mut dst = match image.tile(tx, ty) {
                Some(t) => t.to_vec(),
                None => empty.repeat(TILE_AREA),
            };
            for i in 0..TILE_AREA {
                let mut m = from_u16(a.get(i).copied().unwrap_or(0)) * opacity;
                if m <= 0.0 {
                    continue;
                }
                let mut color = None;
                if let Some(f) = src.per_texel {
                    let (x, y) = (tx * TILE + i as u32 % TILE, ty * TILE + i as u32 / TILE);
                    match f(x, y) {
                        Some((c, af)) => {
                            m *= af;
                            color = Some(c);
                        }
                        None => continue,
                    }
                    if m <= 0.0 {
                        continue;
                    }
                }
                let Some(px) = dst.get_mut(i * comps..(i + 1) * comps) else { continue };
                let cov = from_u16(px[n]);
                for k in 0..n {
                    let p = dec(kind, px[k]);
                    let target = match (carried, color) {
                        (Some(t), _) => dec(kind, t.get(i * n + k).copied().unwrap_or(0)),
                        (None, Some(c)) if stencil_color => c.get(k).copied().unwrap_or(0.0),
                        _ => value.get(k).copied().unwrap_or(0.0),
                    };
                    let v = if src.erase { p * (1.0 - m) } else { target * m + p * (1.0 - m) };
                    px[k] = enc(kind, v);
                }
                px[n] = to_u16(if src.erase { cov * (1.0 - m) } else { m + cov * (1.0 - m) });
            }
            Some(((tx, ty), dst))
        })
        .collect();
    for ((tx, ty), data) in updated {
        img.set_tile(tx, ty, Some(std::sync::Arc::new(data)));
    }
}

/// Per-texel override of a stroke: `(x, y)` → (base color, alpha factor), None = no paint.
pub type TexelFn<'a> = &'a (dyn Fn(u32, u32) -> Option<([f32; 3], f32)> + Sync);

/// The stroke being committed: its alpha, touched tiles, opacity cap and mode.
#[derive(Clone, Copy)]
pub struct CommitSource<'a> {
    pub alpha: &'a TileImage,
    pub tiles: &'a BTreeSet<(u32, u32)>,
    pub opacity: f32,
    pub erase: bool,
    /// Projection tool: base color and alpha per texel from a stencil.
    pub per_texel: Option<TexelFn<'a>>,
    /// Clone and smudge: straight values per channel carried by the stroke (replace `values`).
    pub carried: Option<&'a BTreeMap<Channel, TileImage>>,
}

/// Commit a stroke into paint layer `layer` (channels in `values`; erase clears every painted
/// channel of the layer, or only those in `values` when it is not empty).
pub fn commit_layer(pixels: &mut PixelStore, layer: LayerId, size: (u32, u32), values: &BTreeMap<Channel, [f32; 3]>, src: CommitSource) {
    let opacity = src.opacity.clamp(0.0, 1.0);
    let channels: Vec<Channel> = if src.erase {
        let painted: BTreeSet<Channel> = pixels.layers.keys().filter(|(id, _)| *id == layer).map(|(_, c)| *c).collect();
        painted.into_iter().filter(|c| values.is_empty() || values.contains_key(c)).collect()
    } else {
        values.keys().copied().collect()
    };
    for &c in &channels {
        pixels.layer_mut(layer, c, size);
    }
    // The channels blend in parallel too: an 8K stroke touches hundreds of tiles per channel.
    let images: Vec<(Channel, &mut TileImage)> =
        pixels.layers.iter_mut().filter(|((id, c), _)| *id == layer && channels.contains(c)).map(|((_, c), img)| (*c, img)).collect();
    images.into_par_iter().for_each(|(c, img)| {
        let v = values.get(&c).copied().unwrap_or([0.0; 3]);
        let carried = src.carried.and_then(|m| m.get(&c));
        over(img, c.kind(), &v[..c.components()], &CommitSource { opacity, ..src }, c == Channel::BaseColor, carried);
    });
}

/// Commit a stroke into the paint effect `effect` of a mask, painting gray `value`.
pub fn commit_mask(pixels: &mut PixelStore, effect: LayerId, size: (u32, u32), value: f32, src: CommitSource) {
    let img = pixels.mask_mut(effect, size);
    over(img, ChannelKind::Scalar, &[value.clamp(0.0, 1.0)], &src, false, None);
}

/// Commit a stroke into a coverage-only image (1 component), e.g. the quick mask.
pub fn commit_coverage(img: &mut TileImage, src: CommitSource) {
    over(img, ChannelKind::Scalar, &[], &src, false, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src<'a>(alpha: &'a TileImage, tiles: &'a BTreeSet<(u32, u32)>, opacity: f32, erase: bool) -> CommitSource<'a> {
        CommitSource { alpha, tiles, opacity, erase, per_texel: None, carried: None }
    }

    fn alpha(v: f32) -> (TileImage, BTreeSet<(u32, u32)>) {
        let mut a = TileImage::new(64, 64, 1);
        a.set(5, 5, &[to_u16(v)]);
        (a, [(0, 0)].into_iter().collect())
    }

    #[test]
    fn paint_then_paint_then_erase_is_premultiplied_over() {
        let mut px = PixelStore::default();
        let id = LayerId(1);
        let vals: BTreeMap<Channel, [f32; 3]> = [(Channel::Roughness, [0.8; 3])].into_iter().collect();
        let (a, tiles) = alpha(0.5);
        commit_layer(&mut px, id, (64, 64), &vals, src(&a, &tiles, 1.0, false));
        let t = px.layer(id, Channel::Roughness).unwrap().get(5, 5);
        assert!((from_u16(t[0]) - 0.4).abs() < 1e-3, "premul v·a");
        assert!((from_u16(t[1]) - 0.5).abs() < 1e-3, "coverage a");
        commit_layer(&mut px, id, (64, 64), &vals, src(&a, &tiles, 1.0, false));
        let t = px.layer(id, Channel::Roughness).unwrap().get(5, 5);
        assert!((from_u16(t[1]) - 0.75).abs() < 1e-3);
        assert!((from_u16(t[0]) - 0.6).abs() < 1e-3);
        commit_layer(&mut px, id, (64, 64), &BTreeMap::new(), src(&a, &tiles, 1.0, true));
        let t = px.layer(id, Channel::Roughness).unwrap().get(5, 5);
        assert!((from_u16(t[1]) - 0.375).abs() < 1e-3);
        assert!((from_u16(t[0]) - 0.3).abs() < 1e-3);
    }

    #[test]
    fn signed_height_and_stroke_opacity() {
        let mut px = PixelStore::default();
        let id = LayerId(2);
        let vals: BTreeMap<Channel, [f32; 3]> = [(Channel::Height, [-0.5; 3])].into_iter().collect();
        let (a, tiles) = alpha(1.0);
        commit_layer(&mut px, id, (64, 64), &vals, src(&a, &tiles, 0.5, false));
        let t = px.layer(id, Channel::Height).unwrap().get(5, 5);
        assert!((from_u16_signed(t[0]) + 0.25).abs() < 1e-3);
        assert!((from_u16(t[1]) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn mask_commit_and_untouched_tiles_stay_shared() {
        let mut px = PixelStore::default();
        let (a, tiles) = alpha(1.0);
        commit_mask(&mut px, LayerId(9), (64, 64), 1.0, src(&a, &tiles, 1.0, false));
        let before = px.clone();
        commit_mask(&mut px, LayerId(9), (64, 64), 0.0, src(&a, &tiles, 1.0, true));
        assert_eq!(from_u16(before.masks[&LayerId(9)].get(5, 5)[1]), 1.0);
        assert_eq!(px.masks[&LayerId(9)].get(5, 5)[1], 0);
    }

    #[test]
    fn carried_values_replace_the_tool_value_and_coverage_images_work() {
        let mut px = PixelStore::default();
        let (a, tiles) = alpha(1.0);
        let mut carried = TileImage::new(64, 64, 1);
        carried.set(5, 5, &[to_u16(0.25)]);
        let carried: BTreeMap<Channel, TileImage> = [(Channel::Roughness, carried)].into_iter().collect();
        let vals: BTreeMap<Channel, [f32; 3]> = [(Channel::Roughness, [0.8; 3])].into_iter().collect();
        commit_layer(&mut px, LayerId(3), (64, 64), &vals, CommitSource { carried: Some(&carried), ..src(&a, &tiles, 1.0, false) });
        assert!((from_u16(px.layer(LayerId(3), Channel::Roughness).unwrap().get(5, 5)[0]) - 0.25).abs() < 1e-3);
        let mut cov = TileImage::new(64, 64, 1);
        commit_coverage(&mut cov, src(&a, &tiles, 0.5, false));
        assert!((from_u16(cov.get(5, 5)[0]) - 0.5).abs() < 1e-3);
    }
}
