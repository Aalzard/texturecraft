//! A polygon fill box being dragged: what it filled so far and what it needs to refill each move.

use std::collections::{BTreeMap, BTreeSet};

use texturecraft_geom::glam::Vec2;
use texturecraft_paint::EyeDepth;
use texturecraft_texel::TileImage;

use crate::session::Document;
use crate::triangle_partition::TrianglePartition;

/// A polygon fill box being dragged: the fill shows live and becomes one undo step when the drag ends
pub(crate) struct PolygonFillDrag {
    /// The document before the drag, refilled each time the selection changes
    pub before: Document,
    /// The quick mask's coverage before the drag, when the drag paints the quick mask
    pub quick_mask_before: Option<BTreeMap<usize, TileImage>>,
    /// The triangles filled so far
    pub filled: BTreeSet<u32>,
    /// What the camera sees and the viewport size it was rendered for (3D view, visible polygons only)
    pub depth: Option<(EyeDepth, Vec2)>,
    /// The polygons the box tests
    pub polygons: TrianglePartition,
    /// What the fill mode fills together
    pub fill_groups: TrianglePartition,
    /// The fill failed (reported once): the rest of the drag fills nothing
    pub failed: bool,
}
