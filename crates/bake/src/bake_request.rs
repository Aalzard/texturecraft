use crate::{HighPoly, SkewMap};
use texturecraft_core::BakeSettings;
use texturecraft_geom::{Bvh, Mesh};

#[derive(Clone, Copy, Debug)]
pub struct BakeRequest<'a> {
    pub mesh: &'a Mesh,
    pub bvh: &'a Bvh,
    pub material: usize,
    pub settings: &'a BakeSettings,
    pub high: Option<HighPoly<'a>>,
    pub cage: Option<&'a Mesh>,
    pub skew: Option<&'a SkewMap>,
    /// UV atlas grid; each tile uses the configured output resolution
    pub tiles: (u32, u32),
}
