//! The baked curvature map at the detail sizes of curvature generators, built once per map and layout.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

use rayon::prelude::*;
use texturecraft_core::{BakedMap, BlockScales, CurvatureDetail};
use texturecraft_texel::dilate::{DilationMap, NONE};

use crate::curvature_size_level::{CurvatureSizeLevel, DEVIATION_SCALE};
use crate::eval::{Inputs, blur};

/// The curvature map padded past its UV islands and blurred at each size of [`CurvatureDetail::SIZES`] after the first.
pub struct CurvatureSizes {
    /// One per blurred size, built once a generator gives that size a weight above 0.
    pub levels: Vec<Option<Arc<CurvatureSizeLevel>>>,
}

/// The bake map size, its UV tile block and the working texture size a cache entry was built for.
type CacheKey = ((u32, u32), (u32, u32), (u32, u32));

/// Built sizes by the curvature data they come from, their layout and the tiles' scales.
type CacheEntry = (Weak<Vec<f32>>, CacheKey, BlockScales, Arc<CurvatureSizes>);

static CACHE: Mutex<Vec<CacheEntry>> = Mutex::new(Vec::new());

impl CurvatureSizes {
    /// The sizes of the texture set `inp` composites, holding at least those `needed` marks (one flag per blurred size); None without a baked curvature map or a needed size.
    pub fn of(inp: &Inputs, needed: &[bool]) -> Option<Arc<CurvatureSizes>> {
        let curvature = inp.baked.maps.get(&BakedMap::Curvature)?;
        let (width, height) = (inp.baked.resolution, inp.baked.height(BakedMap::Curvature));
        if !needed.contains(&true) || width == 0 || height == 0 {
            return None;
        }

        let grid = inp.project.texture_set(inp.ts).ok().and_then(|set| set.tile_grid());
        let block = grid.map_or((width, height), |grid| (width / grid.cols.max(1), height / grid.rows.max(1)));
        let key = ((width, height), block, (inp.gbuf.width, inp.gbuf.height));
        let built = cached(curvature, key, &inp.block_scales);
        if let Some(sizes) = built.as_ref().filter(|sizes| sizes.holds(needed)) {
            return Some(sizes.clone());
        }

        // Built without the cache locked: the build runs parallel work, and a thread waiting on it may composite a tile that asks the cache again
        let covered = covered_texels(inp, (width, height));
        let sizes = CurvatureSizes::build(curvature, (width, height), &covered, block, needed, built.as_deref());
        Some(store(curvature, key, &inp.block_scales, sizes))
    }

    /// Whether every size `needed` marks is built.
    fn holds(&self, needed: &[bool]) -> bool {
        needed.iter().zip(&self.levels).all(|(needs, level)| !needs || level.is_some())
    }

    /// Pad `curvature` from its `covered` texels within each UV tile `block`, then blur each block at the sizes `needed` marks; sizes already in `built` are kept as they are.
    fn build(
        curvature: &[f32],
        (width, height): (u32, u32),
        covered: &[bool],
        block: (u32, u32),
        needed: &[bool],
        built: Option<&CurvatureSizes>,
    ) -> CurvatureSizes {
        let padding = DilationMap::in_rectangular_blocks(covered, width, height, None, block);
        let deviation: Vec<f32> =
            padding.src.par_iter().map(|&source| if source == NONE { 0.0 } else { curvature.get(source as usize).copied().unwrap_or(0.5) - 0.5 }).collect();
        let (block_width, block_height) = (block.0.max(1).min(width), block.1.max(1).min(height));

        let levels = CurvatureDetail::SIZES
            .iter()
            .skip(1)
            .enumerate()
            .map(|(level, size)| {
                if let Some(existing) = built.and_then(|sizes| sizes.levels.get(level)).and_then(Option::clone) {
                    return Some(existing);
                }
                if !needed.get(level).copied().unwrap_or(false) {
                    return None;
                }

                // Sizes are texels at 2K; a blur this wide holds nothing finer than half its radius, so it is stored that much coarser
                let radius = (size * block_width as f32 / 2048.0).round().max(1.0) as u32;
                let step = (radius / 2).max(1).next_power_of_two();

                let mut blurred = deviation.clone();
                for block_y in (0..height).step_by(block_height as usize) {
                    for block_x in (0..width).step_by(block_width as usize) {
                        let region = (block_x, block_y, block_width.min(width - block_x), block_height.min(height - block_y));
                        blur_block(&mut blurred, width, region, radius);
                    }
                }
                Some(Arc::new(coarsen(&blurred, (width, height), step)))
            })
            .collect();
        CurvatureSizes { levels }
    }
}

/// The cached sizes of `curvature` for layout `key` and tile `scales`, if any.
fn cached(curvature: &Arc<Vec<f32>>, key: CacheKey, scales: &BlockScales) -> Option<Arc<CurvatureSizes>> {
    let mut cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.retain(|(source, ..)| source.strong_count() > 0);
    cache.iter().find(|entry| is_entry_for(entry, curvature, key, scales)).map(|(.., sizes)| sizes.clone())
}

/// Cache `sizes`, keeping the sizes another thread cached meanwhile, and return what the cache holds now.
fn store(curvature: &Arc<Vec<f32>>, key: CacheKey, scales: &BlockScales, sizes: CurvatureSizes) -> Arc<CurvatureSizes> {
    let mut cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some((.., cached_sizes)) = cache.iter_mut().find(|entry| is_entry_for(entry, curvature, key, scales)) else {
        let sizes = Arc::new(sizes);
        cache.push((Arc::downgrade(curvature), key, scales.clone(), sizes.clone()));
        return sizes;
    };

    let levels = cached_sizes.levels.iter().zip(sizes.levels).map(|(cached, built)| cached.clone().or(built)).collect();
    *cached_sizes = Arc::new(CurvatureSizes { levels });
    cached_sizes.clone()
}

/// Whether cache `entry` holds the sizes of `curvature` for layout `key` and tile `scales`.
fn is_entry_for((source, cached_key, cached_scales, _): &CacheEntry, curvature: &Arc<Vec<f32>>, key: CacheKey, scales: &BlockScales) -> bool {
    source.upgrade().is_some_and(|data| Arc::ptr_eq(&data, curvature)) && *cached_key == key && cached_scales == scales
}

/// The bake atlas texels (`width × height`) that the covered working texels of `inp` land on.
fn covered_texels(inp: &Inputs, (width, height): (u32, u32)) -> Vec<bool> {
    let working_size = (inp.gbuf.width, inp.gbuf.height);
    let covered: Vec<AtomicBool> = (0..width * height).map(|_| AtomicBool::new(false)).collect();
    (0..working_size.0 * working_size.1).into_par_iter().filter(|texel| inp.gbuf.covered(*texel as usize)).for_each(|texel| {
        let uv = [((texel % working_size.0) as f32 + 0.5) / working_size.0 as f32, ((texel / working_size.0) as f32 + 0.5) / working_size.1 as f32];
        if let Some(baked_uv) = inp.block_scales.bake_uv(uv, working_size) {
            let (baked_x, baked_y) = ((baked_uv[0] * width as f32) as u32, (baked_uv[1] * height as f32) as u32);
            if let Some(slot) = covered.get((baked_y.min(height - 1) * width + baked_x.min(width - 1)) as usize) {
                slot.store(true, Ordering::Relaxed);
            }
        }
    });
    covered.into_iter().map(AtomicBool::into_inner).collect()
}

/// Blur the `(x0, y0, w, h)` block of `data` (`width` texels per row) in place, reading nothing outside it.
fn blur_block(data: &mut [f32], width: u32, (x0, y0, w, h): (u32, u32, u32, u32), radius: u32) {
    let mut block: Vec<f32> =
        (y0..y0 + h).flat_map(|y| (x0..x0 + w).map(move |x| (y * width + x) as usize)).map(|index| data.get(index).copied().unwrap_or(0.0)).collect();
    blur(&mut block, w as usize, h as usize, radius as usize);

    for (row, values) in block.chunks(w as usize).enumerate() {
        let start = ((y0 + row as u32) * width + x0) as usize;
        if let Some(target) = data.get_mut(start..start + values.len()) {
            target.copy_from_slice(values);
        }
    }
}

/// `data` averaged over `step × step` squares (the last ones clipped to the map).
fn coarsen(data: &[f32], (width, height): (u32, u32), step: u32) -> CurvatureSizeLevel {
    let (coarse_width, coarse_height) = (width.div_ceil(step), height.div_ceil(step));
    let deviation = (0..coarse_width * coarse_height)
        .into_par_iter()
        .map(|cell| {
            let (cell_x, cell_y) = (cell % coarse_width, cell / coarse_width);
            let columns = cell_x * step..((cell_x + 1) * step).min(width);
            let rows = cell_y * step..((cell_y + 1) * step).min(height);
            let count = (columns.len() * rows.len()).max(1) as f32;
            let texels = rows.flat_map(|y| columns.clone().map(move |x| (y * width + x) as usize));
            let mean = texels.map(|index| data.get(index).copied().unwrap_or(0.0)).sum::<f32>() / count;
            (mean * DEVIATION_SCALE).round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
        })
        .collect();
    CurvatureSizeLevel { step, width: coarse_width, height: coarse_height, deviation }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_keeps_an_island_border_from_reading_flat() {
        let map_size: u32 = 64;
        let left_half = |texel: u32| texel % map_size < map_size / 2;
        let curvature: Vec<f32> = (0..map_size * map_size).map(|texel| if left_half(texel) { 1.0 } else { 0.0 }).collect();
        let covered: Vec<bool> = (0..map_size * map_size).map(left_half).collect();
        let shape = (map_size, map_size);
        let sizes = CurvatureSizes::build(&curvature, shape, &covered, shape, &[true; CurvatureDetail::BLURRED_SIZES], None);

        // The last covered column, halfway down
        let (border_x, border_y) = (map_size / 2 - 1, map_size / 2);
        for level in sizes.levels.iter().flatten() {
            assert!((level.sample(border_x, border_y) - 0.5).abs() < 0.01, "the border keeps the island's curvature, not the empty texels beside it");
        }
    }

    #[test]
    fn sizes_built_on_demand_match_a_full_build() {
        let map_size: u32 = 64;
        let curvature: Vec<f32> =
            (0..map_size * map_size).map(|texel| 0.5 + 0.4 * ((texel % map_size) as f32 * 0.3).sin() * ((texel / map_size) as f32 * 0.2).cos()).collect();
        let covered = vec![true; (map_size * map_size) as usize];
        let shape = (map_size, map_size);
        let all = vec![true; CurvatureDetail::BLURRED_SIZES];
        let full = CurvatureSizes::build(&curvature, shape, &covered, shape, &all, None);

        let even: Vec<bool> = (0..all.len()).map(|level| level % 2 == 0).collect();
        let partial = CurvatureSizes::build(&curvature, shape, &covered, shape, &even, None);
        assert!(partial.levels.iter().zip(&even).all(|(level, needed)| level.is_some() == *needed), "only the needed sizes are built");

        let completed = CurvatureSizes::build(&curvature, shape, &covered, shape, &all, Some(&partial));
        for ((built, expected), earlier) in completed.levels.iter().zip(&full.levels).zip(&partial.levels) {
            assert_eq!(built.as_ref().map(|level| &level.deviation), expected.as_ref().map(|level| &level.deviation), "the same values either way");
            if let (Some(built), Some(earlier)) = (built, earlier) {
                assert!(Arc::ptr_eq(built, earlier), "sizes already built are kept, not rebuilt");
            }
        }
    }
}
