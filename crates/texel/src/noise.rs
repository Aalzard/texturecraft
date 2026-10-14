//! Deterministic 3D noise (integer hashing, so results are identical on every platform).

/// PCG3D hash (Jarzynski & Olano 2020).
#[inline]
pub fn pcg3d(x: u32, y: u32, z: u32) -> [u32; 3] {
    let mut v =
        [x.wrapping_mul(1664525).wrapping_add(1013904223), y.wrapping_mul(1664525).wrapping_add(1013904223), z.wrapping_mul(1664525).wrapping_add(1013904223)];
    v[0] = v[0].wrapping_add(v[1].wrapping_mul(v[2]));
    v[1] = v[1].wrapping_add(v[2].wrapping_mul(v[0]));
    v[2] = v[2].wrapping_add(v[0].wrapping_mul(v[1]));
    v = v.map(|a| a ^ (a >> 16));
    v[0] = v[0].wrapping_add(v[1].wrapping_mul(v[2]));
    v[1] = v[1].wrapping_add(v[2].wrapping_mul(v[0]));
    v[2] = v[2].wrapping_add(v[0].wrapping_mul(v[1]));
    v
}

#[inline]
fn cell(p: f32) -> (i32, f32) {
    let f = p.floor();
    (f as i32, p - f)
}

#[inline]
fn hash01(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    pcg3d(x as u32 ^ seed.wrapping_mul(0x9E37_79B9), y as u32, z as u32)[0] as f32 / u32::MAX as f32
}

#[inline]
fn smooth(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Value noise in 0..1.
pub fn value(p: [f32; 3], seed: u32) -> f32 {
    let (ix, fx) = cell(p[0]);
    let (iy, fy) = cell(p[1]);
    let (iz, fz) = cell(p[2]);
    let (sx, sy, sz) = (smooth(fx), smooth(fy), smooth(fz));
    let h = |dx: i32, dy: i32, dz: i32| hash01(ix + dx, iy + dy, iz + dz, seed);
    lerp(
        lerp(lerp(h(0, 0, 0), h(1, 0, 0), sx), lerp(h(0, 1, 0), h(1, 1, 0), sx), sy),
        lerp(lerp(h(0, 0, 1), h(1, 0, 1), sx), lerp(h(0, 1, 1), h(1, 1, 1), sx), sy),
        sz,
    )
}

fn grad(x: i32, y: i32, z: i32, seed: u32, f: [f32; 3]) -> f32 {
    let h = pcg3d(x as u32 ^ seed.wrapping_mul(0x85EB_CA6B), y as u32, z as u32)[1] % 12;
    let (a, b) = match h {
        0 => (f[0], f[1]),
        1 => (-f[0], f[1]),
        2 => (f[0], -f[1]),
        3 => (-f[0], -f[1]),
        4 => (f[0], f[2]),
        5 => (-f[0], f[2]),
        6 => (f[0], -f[2]),
        7 => (-f[0], -f[2]),
        8 => (f[1], f[2]),
        9 => (-f[1], f[2]),
        10 => (f[1], -f[2]),
        _ => (-f[1], -f[2]),
    };
    a + b
}

/// Gradient (Perlin) noise in about 0..1.
pub fn perlin(p: [f32; 3], seed: u32) -> f32 {
    let (ix, fx) = cell(p[0]);
    let (iy, fy) = cell(p[1]);
    let (iz, fz) = cell(p[2]);
    let (sx, sy, sz) = (smooth(fx), smooth(fy), smooth(fz));
    let g = |dx: i32, dy: i32, dz: i32| grad(ix + dx, iy + dy, iz + dz, seed, [fx - dx as f32, fy - dy as f32, fz - dz as f32]);
    let v = lerp(
        lerp(lerp(g(0, 0, 0), g(1, 0, 0), sx), lerp(g(0, 1, 0), g(1, 1, 0), sx), sy),
        lerp(lerp(g(0, 0, 1), g(1, 0, 1), sx), lerp(g(0, 1, 1), g(1, 1, 1), sx), sy),
        sz,
    );
    (v * 0.5 + 0.5).clamp(0.0, 1.0)
}

/// Worley (cellular) noise: distance to the nearest feature point, about 0..1.
pub fn worley(p: [f32; 3], seed: u32) -> f32 {
    cellular(p, seed).0
}

/// Cellular noise: distances to the nearest and second-nearest feature points (each about
/// 0..1) and a random 0..1 id of the nearest point's cell (for per-cell variation).
pub fn cellular(p: [f32; 3], seed: u32) -> (f32, f32, f32) {
    let (ix, fx) = cell(p[0]);
    let (iy, fy) = cell(p[1]);
    let (iz, fz) = cell(p[2]);
    let (mut f1, mut f2, mut id) = (f32::MAX, f32::MAX, 0u32);
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let h = pcg3d((ix + dx) as u32 ^ seed.wrapping_mul(0xC2B2_AE35), (iy + dy) as u32, (iz + dz) as u32);
                let o = h.map(|v| v as f32 / u32::MAX as f32);
                let d = [dx as f32 + o[0] - fx, dy as f32 + o[1] - fy, dz as f32 + o[2] - fz];
                let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                if d2 < f1 {
                    (f2, f1, id) = (f1, d2, h[0] ^ h[1]);
                } else if d2 < f2 {
                    f2 = d2;
                }
            }
        }
    }
    (f1.sqrt().min(1.0), f2.sqrt().min(1.0), id as f32 / u32::MAX as f32)
}

/// Random 0..1 value of an integer lattice cell (per-brick or per-tile variation).
pub fn cell_hash(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    hash01(x, y, z, seed)
}

/// Fractal sum of `octaves` layers of `base` noise, normalized to 0..1.
pub fn fbm(p: [f32; 3], octaves: u32, seed: u32, base: fn([f32; 3], u32) -> f32) -> f32 {
    let (mut sum, mut amp, mut norm, mut q) = (0.0, 0.5, 0.0, p);
    for o in 0..octaves.clamp(1, 12) {
        sum += amp * base(q, seed.wrapping_add(o * 31));
        norm += amp;
        q = q.map(|v| v * 2.0 + 17.13);
        amp *= 0.5;
    }
    sum / norm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noises_are_deterministic_and_in_range() {
        for i in 0..500 {
            let p = [i as f32 * 0.37, i as f32 * 0.11 - 3.0, (i % 17) as f32 * 0.53];
            for f in [value, perlin, worley] {
                let a = f(p, 3);
                assert!((0.0..=1.0).contains(&a), "{a}");
                assert_eq!(a, f(p, 3));
            }
            let a = fbm(p, 5, 1, value);
            assert!((0.0..=1.0).contains(&a));
            let (f1, f2, id) = cellular(p, 3);
            assert!(f1 <= f2 && (0.0..=1.0).contains(&f2) && (0.0..=1.0).contains(&id), "{f1} {f2} {id}");
        }
    }

    #[test]
    fn seeds_change_the_pattern() {
        let p = [1.3, 2.7, 0.4];
        assert_ne!(value(p, 1), value(p, 2));
    }

    #[test]
    fn value_noise_is_continuous() {
        let a = value([0.5, 0.5, 0.5], 0);
        let b = value([0.5001, 0.5, 0.5], 0);
        assert!((a - b).abs() < 1e-2);
    }
}
