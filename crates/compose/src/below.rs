//! A per-tile cache of the composited stack below one "hot" layer (the layer being painted or
//! edited), so recompositing a tile only evaluates that layer and the layers above it. The
//! cache is keyed by everything the cached part reads; any change to it empties the cache, so
//! results are always identical to a full evaluation.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::Arc;

use texturecraft_core::{BakedMap, Channel, Layer, LayerId, LayerKind, Project, TextureSet};
use texturecraft_texel::TileImage;

use crate::eval::{Inputs, PaintTarget};
use crate::store::{BakedMaps, ImageStore, PixelStore};

/// Default byte budget of a [`BelowCache`].
pub const BELOW_BUDGET: usize = 256 << 20;

/// See the module docs. One per texture set.
#[derive(Debug)]
pub struct BelowCache {
    hot: Option<LayerId>,
    key: Option<StackKey>,
    tiles: HashMap<(u32, u32), Vec<f32>>,
    /// Insertion order, oldest first (eviction beyond the budget).
    order: VecDeque<(u32, u32)>,
    bytes: usize,
    /// Byte budget; the oldest tiles are dropped beyond it.
    pub budget: usize,
    /// Tiles of the whole pass a caller splits into several composites (0: each composite is a
    /// pass). A pass larger than the cache records only what fits.
    pub pass: usize,
}

impl Default for BelowCache {
    fn default() -> Self {
        BelowCache { hot: None, key: None, tiles: HashMap::new(), order: VecDeque::new(), bytes: 0, budget: BELOW_BUDGET, pass: 0 }
    }
}

/// Everything compositing `roots` of a texture set reads: equal keys composite identically.
/// Pixels are compared by tile identity (copy-on-write), so building and comparing keys is cheap.
#[derive(Debug, PartialEq)]
pub struct StackKey {
    /// The texture set without its name, description, stack and bake settings.
    set: TextureSet,
    roots: Vec<LayerId>,
    /// The layers drawn, without names and other display-only fields.
    layers: Vec<Layer>,
    pixels: Vec<(LayerId, Option<Channel>, Shared)>,
    baked: (u32, Vec<(BakedMap, usize)>),
    images: Vec<(String, usize)>,
}

impl StackKey {
    /// The key of `roots` (layers of texture set `ts`, bottom → top) and their effects.
    pub fn new(project: &Project, pixels: &PixelStore, baked: &BakedMaps, images: &ImageStore, ts: usize, roots: &[LayerId]) -> Option<StackKey> {
        let mut set = project.texture_set(ts).ok()?.clone();
        set.name.clear();
        set.description.clear();
        set.stack.clear();
        set.bake = Default::default();
        let tree = project.render_tree(roots);
        let effects: BTreeSet<LayerId> = tree.iter().filter_map(|id| project.layers.get(id)).flat_map(effect_ids).collect();
        let pixels = tree
            .iter()
            .chain(&effects)
            .flat_map(|id| {
                let layer = Channel::ALL.into_iter().filter_map(move |c| pixels.layer(*id, c).map(|img| (*id, Some(c), Shared(img.clone()))));
                layer.chain(pixels.masks.get(id).map(|img| (*id, None, Shared(img.clone()))))
            })
            .collect();
        let mut images: Vec<(String, usize)> = images.iter().map(|(n, i)| (n.clone(), Arc::as_ptr(i) as usize)).collect();
        images.sort();
        Some(StackKey {
            set,
            roots: roots.to_vec(),
            layers: tree.iter().filter_map(|id| project.layers.get(id)).map(render_view).collect(),
            pixels,
            baked: (baked.resolution, baked.maps.iter().map(|(m, d)| (*m, Arc::as_ptr(d) as usize)).collect()),
            images,
        })
    }
}

/// An image compared by tile identity: holding the clone keeps its tiles alive, and a tile
/// rewritten since is a different allocation (copy-on-write), so pointer equality means the
/// same pixels without reading them.
#[derive(Debug)]
struct Shared(TileImage);

impl PartialEq for Shared {
    fn eq(&self, other: &Self) -> bool {
        let (a, b) = (&self.0, &other.0);
        a.width() == b.width()
            && a.height() == b.height()
            && a.comps() == b.comps()
            && (0..a.tiles_y()).all(|ty| {
                (0..a.tiles_x()).all(|tx| match (a.tile_handle(tx, ty), b.tile_handle(tx, ty)) {
                    (None, None) => true,
                    (Some(x), Some(y)) => Arc::ptr_eq(x, y),
                    _ => false,
                })
            })
    }
}

/// Ids of a layer's mask and content effects.
fn effect_ids(l: &Layer) -> impl Iterator<Item = LayerId> + '_ {
    l.mask.iter().flat_map(|m| m.effects.iter()).chain(l.content_effects.iter()).map(|e| e.id)
}

/// A layer as far as compositing is concerned (names, color tags and folding don't matter).
fn render_view(l: &Layer) -> Layer {
    let mut l = l.clone();
    l.name.clear();
    l.color_tag = None;
    if let LayerKind::Group { collapsed, .. } = &mut l.kind {
        *collapsed = false;
    }
    l
}

impl BelowCache {
    /// Cache the stack below `hot` from now on (a different layer empties the cache).
    pub fn set_hot(&mut self, hot: Option<LayerId>) {
        if self.hot != hot {
            self.hot = hot;
            self.clear();
        }
    }

    pub fn clear(&mut self) {
        self.tiles.clear();
        self.order.clear();
        self.bytes = 0;
        self.key = None;
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    pub(crate) fn tile(&self, t: (u32, u32)) -> Option<&[f32]> {
        self.tiles.get(&t).map(Vec::as_slice)
    }

    pub(crate) fn insert(&mut self, t: (u32, u32), data: Vec<f32>) {
        let size = data.len() * std::mem::size_of::<f32>();
        if size > self.budget {
            return;
        }
        while self.bytes + size > self.budget {
            let Some(old) = self.order.pop_front() else { break };
            if let Some(d) = self.tiles.remove(&old) {
                self.bytes = self.bytes.saturating_sub(d.len() * std::mem::size_of::<f32>());
            }
        }
        if let Some(d) = self.tiles.insert(t, data) {
            self.bytes = self.bytes.saturating_sub(d.len() * std::mem::size_of::<f32>());
        } else {
            self.order.push_back(t);
        }
        self.bytes += size;
    }

    /// Where the stack splits for `inp`: layers of the texture set's top level before the
    /// returned index are served from the cache. None when the hot layer is not drawn in the set
    /// or the cached part could change (it reads anchors or holds the live stroke). Empties the
    /// cache when what the cached part reads changed.
    pub(crate) fn prepare(&mut self, inp: &Inputs) -> Option<usize> {
        let hot = self.hot?;
        let p = inp.project;
        let set = p.texture_set(inp.ts).ok()?;
        let split = set.stack.iter().position(|id| p.render_tree(&[*id]).contains(&hot))?;
        let below = set.stack.get(..split)?;
        // Anchors carry values across the split.
        if p.render_tree(&set.stack).iter().any(|id| p.layers.get(id).is_some_and(Layer::reads_anchor)) {
            return None;
        }
        if let Some(pv) = inp.preview {
            let (PaintTarget::Layer(t) | PaintTarget::Mask(t)) = pv.target;
            let tree = p.render_tree(below);
            if tree.contains(&t) || tree.iter().filter_map(|id| p.layers.get(id)).flat_map(effect_ids).any(|e| e == t) {
                return None;
            }
        }
        let key = StackKey::new(p, inp.pixels, inp.baked, inp.images, inp.ts, below)?;
        if self.key.as_ref() != Some(&key) {
            self.clear();
            self.key = Some(key);
        }
        Some(split)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_keeps_to_the_budget_dropping_the_oldest() {
        let mut c = BelowCache { budget: 3 * 16, ..BelowCache::default() };
        for i in 0..5 {
            c.insert((i, 0), vec![0.0; 4]);
        }
        assert_eq!(c.len(), 3);
        assert_eq!(c.bytes(), 48);
        assert!(c.tile((0, 0)).is_none() && c.tile((4, 0)).is_some());
        c.insert((9, 9), vec![0.0; 100]);
        assert!(c.tile((9, 9)).is_none(), "bigger than the whole budget");
        c.set_hot(Some(LayerId(3)));
        assert!(c.is_empty() && c.bytes() == 0);
    }
}
