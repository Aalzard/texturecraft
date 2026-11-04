use std::sync::Arc;
use texturecraft_geom::{BaseColor, Bvh, Mesh};

#[derive(Clone, Copy, Debug)]
pub struct HighPoly<'a> {
    pub mesh: &'a Mesh,
    pub bvh: &'a Bvh,
    pub base_color: &'a [Option<Arc<BaseColor>>],
}
