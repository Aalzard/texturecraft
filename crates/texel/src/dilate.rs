//! UV padding: fill uncovered texels from their nearest covered texel.

use std::collections::VecDeque;

/// No source texel.
pub const NONE: u32 = u32::MAX;

/// For every texel, the index of the covered texel whose value it takes (itself when covered,
/// [`NONE`] when beyond the padding distance). Computed once per UV layout and reused for every
/// dilation of that texture set.
#[derive(Clone, Debug, PartialEq)]
pub struct DilationMap {
    pub width: u32,
    pub height: u32,
    pub src: Vec<u32>,
}

impl DilationMap {
    /// `covered.len()` must be `width × height` (shorter input is treated as uncovered).
    /// `max_dist` limits padding in texels (Chebyshev steps); `None` fills everything reachable.
    pub fn new(covered: &[bool], width: u32, height: u32, max_dist: Option<u32>) -> DilationMap {
        let n = (width as usize).saturating_mul(height as usize);
        let mut src = vec![NONE; n];
        let mut dist = vec![u32::MAX; n];
        let mut queue = VecDeque::new();
        for i in 0..n {
            if covered.get(i).copied().unwrap_or(false) {
                src[i] = i as u32;
                dist[i] = 0;
                queue.push_back(i);
            }
        }
        let limit = max_dist.unwrap_or(u32::MAX);
        let (w, h) = (width as i64, height as i64);
        while let Some(i) = queue.pop_front() {
            let d = dist[i];
            if d >= limit {
                continue;
            }
            let (x, y) = ((i as i64) % w, (i as i64) / w);
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w || ny >= h {
                    continue;
                }
                let j = (ny * w + nx) as usize;
                if dist[j] == u32::MAX {
                    dist[j] = d + 1;
                    src[j] = src[i];
                    queue.push_back(j);
                }
            }
        }
        DilationMap { width, height, src }
    }

    /// Copy padded values in place: `data` holds `comps` components per texel. Texels with no
    /// source keep their value.
    pub fn apply<T: Copy>(&self, data: &mut [T], comps: usize) {
        for (i, &s) in self.src.iter().enumerate() {
            let s = s as usize;
            if s == i || s == NONE as usize {
                continue;
            }
            for c in 0..comps {
                if let Some(v) = data.get(s * comps + c).copied()
                    && let Some(d) = data.get_mut(i * comps + c)
                {
                    *d = v;
                }
            }
        }
    }

    /// The source texel of (x, y), if any.
    pub fn source(&self, x: u32, y: u32) -> Option<(u32, u32)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let s = *self.src.get((y * self.width + x) as usize)?;
        (s != NONE).then(|| (s % self.width, s / self.width))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncovered_texels_take_the_nearest_covered_value() {
        // 5×1 strip, only texel 1 covered.
        let covered = [false, true, false, false, false];
        let m = DilationMap::new(&covered, 5, 1, Some(2));
        assert_eq!(m.src, vec![1, 1, 1, 1, NONE]);
        let mut data = [0u8, 9, 0, 0, 0];
        m.apply(&mut data, 1);
        assert_eq!(data, [9, 9, 9, 9, 0]);
    }

    #[test]
    fn infinite_padding_fills_everything_and_keeps_covered_values() {
        let covered = [true, false, false, false, false, false, false, false, true];
        let m = DilationMap::new(&covered, 3, 3, None);
        assert!(m.src.iter().all(|s| *s != NONE));
        assert_eq!(m.src[0], 0);
        assert_eq!(m.src[8], 8);
        assert_eq!(m.source(2, 2), Some((2, 2)));
        assert_eq!(m.source(9, 9), None);
    }

    #[test]
    fn nothing_covered_is_a_no_op() {
        let m = DilationMap::new(&[], 4, 4, None);
        let mut data = [3u16; 16];
        m.apply(&mut data, 1);
        assert_eq!(data, [3u16; 16]);
    }
}
