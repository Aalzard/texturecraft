/// Divergence of the tangent-space normal map; positive curvature is convex
pub(crate) fn curvature_from_normal(normal: &[f32], covered: &[bool], width: usize, tile_resolution: usize, output: &mut [f32]) {
    use rayon::prelude::*;
    let radius_texels = (tile_resolution / 512).max(1);
    let normal_component = |x: usize, y: usize, component: usize, fallback: f32| match covered.get(y * width + x) {
        Some(true) => normal.get((y * width + x) * 3 + component).copied().unwrap_or(fallback),
        _ => fallback,
    };
    output.par_chunks_mut(width).enumerate().for_each(|(y, row)| {
        for (x, value) in row.iter_mut().enumerate() {
            if covered.get(y * width + x) != Some(&true) {
                continue;
            }
            let (center_x, center_y) = (normal_component(x, y, 0, 0.5), normal_component(x, y, 1, 0.5));
            let (left, right) = (
                normal_component(x.saturating_sub(radius_texels), y, 0, center_x),
                if x + radius_texels < width { normal_component(x + radius_texels, y, 0, center_x) } else { center_x },
            );
            let (up, down) = (
                normal_component(x, y.saturating_sub(radius_texels), 1, center_y),
                if y + radius_texels < width { normal_component(x, y + radius_texels, 1, center_y) } else { center_y },
            );
            *value = (0.5 + (right - left) + (up - down)).clamp(0.0, 1.0);
        }
    });
}
