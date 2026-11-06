use serde::{Deserialize, Serialize};

use super::curvature_mode::CurvatureMode;

/// How a curvature generator reads the curvature map: its mode and how much each detail size adds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CurvatureDetail {
    pub mode: CurvatureMode,
    /// Weight (0..1) of each size in [`CurvatureDetail::SIZES`], finest first.
    pub weights: [f32; CurvatureDetail::SIZES.len()],
}

impl CurvatureDetail {
    /// Detail sizes as blur radii in texels at 2K: the map itself, then doubling radii.
    pub const SIZES: [f32; 7] = [0.0, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0];
}

impl Default for CurvatureDetail {
    fn default() -> Self {
        CurvatureDetail { mode: CurvatureMode::Edges, weights: [1.0, 0.85, 0.7, 0.55, 0.4, 0.25, 0.15] }
    }
}
