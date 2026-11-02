//! Skew correction: a greyscale map over the UV atlas that turns the transfer rays of a
//! high-poly bake from the averaged cage direction towards the low poly's own normal. Where the
//! averaged direction leans away from a surface (the classic "skewed" details on a bevelled
//! edge or a cylinder's cap), painting the map white there sends the rays straight along the
//! surface normal instead, and grey values blend the two.
//!
//! Ported from SurfaceCraft's baker (`crates/bake`, MIT OR Apache-2.0), adapted to this crate's
//! [`crate::maps::Ctx`] rays. Like the averaged cage it only changes rays that start from the
//! built-in distance cage: a custom cage mesh sets its own directions and ignores the map.

use std::collections::HashMap;

use texturecraft_geom::Mesh;
use texturecraft_geom::glam::{Vec2, Vec3};

use crate::{BakeError, MAX_TEXELS};

/// Most dabs one [`SkewMap::paint`] call places.
pub const MAX_DABS: usize = 200_000;

/// Most texels one [`SkewMap::paint`] call may visit, summed over its dabs (about 70 ms of work):
/// a big brush on a big map costs per dab what a whole-map pass costs, so dabs alone don't bound it.
pub const MAX_PAINT_TEXELS: u64 = 1 << 26;

/// Edges steeper than this cosine between two faces count as hard (about 30 degrees).
const HARD_EDGE_COS: f32 = 0.866;

/// A skew map over the whole UV atlas.
#[derive(Clone, Debug, PartialEq)]
pub struct SkewMap {
    pub width: u32,
    pub height: u32,
    /// `width * height` values in 0..=1, row 0 at the top (v = 1): 0 keeps a ray on the cage's
    /// averaged direction, 1 turns it fully onto the base normal.
    pub values: Vec<f32>,
    /// Edge protection: the correction fades out within this fraction of the mesh diagonal of a
    /// hard edge (0: off).
    pub edge_distance: f32,
    /// Edge protection's falloff exponent: 1 is linear, more holds the correction back longer.
    pub edge_contrast: f32,
    /// Correct towards each triangle's flat normal instead of the interpolated vertex normal.
    pub per_triangle: bool,
}

impl SkewMap {
    /// A map of one value everywhere.
    pub fn constant(value: f32) -> SkewMap {
        SkewMap { width: 1, height: 1, values: vec![value], edge_distance: 0.0, edge_contrast: 1.0, per_triangle: false }
    }

    /// Refuses maps whose size doesn't match their values, non-finite numbers and absurd sizes.
    pub fn validate(&self) -> Result<(), BakeError> {
        let invalid = |m: &str| Err(BakeError::Invalid(m.into()));
        let texels = u64::from(self.width) * u64::from(self.height);
        if self.width == 0 || self.height == 0 || texels > MAX_TEXELS || self.values.len() as u64 != texels {
            return invalid("the skew map's size doesn't match its values");
        }
        if !(self.edge_distance.is_finite() && self.edge_distance >= 0.0 && self.edge_contrast.is_finite() && self.edge_contrast > 0.0) {
            return invalid("the skew edge distance must be 0 or more and the contrast above 0");
        }
        if self.values.iter().any(|v| !v.is_finite()) {
            return invalid("the skew map has a value that is not a finite number");
        }
        Ok(())
    }

    /// The map resampled (bilinear) to `width` × `height`; the other settings carry over.
    pub fn resized(&self, width: u32, height: u32) -> Result<SkewMap, BakeError> {
        let texels = u64::from(width) * u64::from(height);
        if width == 0 || height == 0 || texels > MAX_TEXELS {
            return Err(BakeError::Invalid(format!("a {width} × {height} skew map is outside the supported size")));
        }
        let mut values = Vec::with_capacity(texels as usize);
        for y in 0..height {
            let v = 1.0 - (y as f32 + 0.5) / height as f32;
            for x in 0..width {
                values.push(self.at(Vec2::new((x as f32 + 0.5) / width as f32, v)));
            }
        }
        Ok(SkewMap { width, height, values, ..self.clone() })
    }

    /// Paint round dabs along `points` (atlas uv): `value` is blended in with `strength` (0..=1),
    /// fully inside `hardness` (0..=1) of `radius` (in uv units) and fading to nothing at the rim.
    /// Dabs are placed a quarter radius apart along each segment; `MAX_DABS` bounds one call.
    pub fn paint(&mut self, points: &[Vec2], radius: f32, value: f32, hardness: f32, strength: f32) -> Result<(), BakeError> {
        let ok = radius.is_finite() && radius > 0.0 && value.is_finite() && hardness.is_finite() && strength.is_finite();
        if !ok || points.iter().any(|p| !p.is_finite()) {
            return Err(BakeError::Invalid("the skew brush needs finite numbers and a radius above 0".into()));
        }
        self.validate()?;
        let (value, hardness, strength) = (value.clamp(0.0, 1.0), hardness.clamp(0.0, 0.999), strength.clamp(0.0, 1.0));
        let step = (radius * 0.25).max(1e-5);
        let mut dabs = Vec::new();
        for (i, p) in points.iter().enumerate() {
            match i.checked_sub(1).and_then(|j| points.get(j)) {
                Some(prev) => {
                    let n = ((*p - *prev).length() / step).ceil().clamp(1.0, MAX_DABS as f32) as usize;
                    dabs.extend((1..=n).map(|k| *prev + (*p - *prev) * (k as f32 / n as f32)));
                }
                None => dabs.push(*p),
            }
            if dabs.len() > MAX_DABS {
                return Err(BakeError::Invalid(format!("a skew stroke is limited to {MAX_DABS} dabs; use a larger radius or fewer points")));
            }
        }
        let (w, h) = (self.width as usize, self.height as usize);
        // The texel box of a dab, row 0 at the top (v = 1).
        let span = |c: Vec2| {
            let x0 = (((c.x - radius) * w as f32).floor().max(0.0)) as usize;
            let x1 = (((c.x + radius) * w as f32).ceil().min(w as f32)).max(0.0) as usize;
            let y0 = ((((1.0 - c.y) - radius) * h as f32).floor().max(0.0)) as usize;
            let y1 = ((((1.0 - c.y) + radius) * h as f32).ceil().min(h as f32)).max(0.0) as usize;
            (x0, x1, y0, y1)
        };
        // Refuse a stroke that would take too long before the first texel changes.
        let work: u64 = dabs
            .iter()
            .map(|c| {
                let (x0, x1, y0, y1) = span(*c);
                x1.saturating_sub(x0) as u64 * y1.saturating_sub(y0) as u64
            })
            .fold(0, u64::saturating_add);
        if work > MAX_PAINT_TEXELS {
            return Err(BakeError::Invalid(format!(
                "this stroke would visit {work} texels (limit {MAX_PAINT_TEXELS}); use a smaller radius, a smaller map or a shorter stroke"
            )));
        }
        for c in dabs {
            let (x0, x1, y0, y1) = span(c);
            for y in y0..y1 {
                let dv = (1.0 - (y as f32 + 0.5) / h as f32) - c.y;
                for x in x0..x1 {
                    let du = (x as f32 + 0.5) / w as f32 - c.x;
                    let t = (du * du + dv * dv).sqrt() / radius;
                    if t >= 1.0 {
                        continue;
                    }
                    let fall = if t <= hardness {
                        1.0
                    } else {
                        let k = (1.0 - t) / (1.0 - hardness);
                        k * k * (3.0 - 2.0 * k)
                    };
                    if let Some(v) = self.values.get_mut(y * w + x) {
                        *v += (value - *v) * fall * strength;
                    }
                }
            }
        }
        Ok(())
    }

    /// The map at atlas `uv`: bilinear, edges clamped.
    pub(crate) fn at(&self, uv: Vec2) -> f32 {
        let (w, h) = (self.width.max(1) as usize, self.height.max(1) as usize);
        if !uv.is_finite() {
            return 0.0;
        }
        let x = (uv.x.clamp(0.0, 1.0) * w as f32 - 0.5).clamp(0.0, w as f32 - 1.0);
        let y = ((1.0 - uv.y).clamp(0.0, 1.0) * h as f32 - 0.5).clamp(0.0, h as f32 - 1.0);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let v = |xx: usize, yy: usize| self.values.get(yy * w + xx).copied().unwrap_or(0.0);
        let top = v(x0, y0) * (1.0 - fx) + v(x1, y0) * fx;
        let bottom = v(x0, y1) * (1.0 - fx) + v(x1, y1) * fx;
        (top * (1.0 - fy) + bottom * fy).clamp(0.0, 1.0)
    }
}

/// A skew map ready for a bake: the map plus what edge protection needs.
pub(crate) struct Prepared<'a> {
    map: &'a SkewMap,
    /// Per triangle, which of its edges (v0-v1, v1-v2, v2-v0) are hard; only with edge protection.
    hard: Vec<[bool; 3]>,
    diag: f32,
}

impl<'a> Prepared<'a> {
    pub(crate) fn new(map: &'a SkewMap, mesh: &Mesh, diag: f32) -> Result<Prepared<'a>, BakeError> {
        map.validate()?;
        let hard = if map.edge_distance > 0.0 { hard_edges(mesh) } else { Vec::new() };
        Ok(Prepared { map, hard, diag })
    }

    /// The ray direction `up` (unit, away from the surface) turned towards the base normal: `nrm`
    /// is the interpolated vertex normal, `geo` the flat one, `uv` the atlas position of the
    /// sample at `pos` on triangle `tri`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn correct(&self, mesh: &Mesh, tri: u32, pos: Vec3, uv: Vec2, up: Vec3, nrm: Vec3, geo: Vec3) -> Vec3 {
        let amount = self.map.at(uv);
        if amount <= 0.0 {
            return up;
        }
        let protect = match (self.hard.get(tri as usize), mesh.tri(tri as usize)) {
            (Some(edges), Some(c)) => {
                let near = (0..3)
                    .filter(|&k| edges[k])
                    .filter_map(|k| {
                        let a = mesh.positions.get(*c.get(k)?)?;
                        let b = mesh.positions.get(*c.get((k + 1) % 3)?)?;
                        Some(segment_distance(pos, *a, *b))
                    })
                    .fold(f32::INFINITY, f32::min);
                (near / (self.map.edge_distance * self.diag).max(1e-9)).clamp(0.0, 1.0).powf(self.map.edge_contrast)
            }
            _ => 1.0,
        };
        let base = if self.map.per_triangle { geo } else { nrm };
        let t = amount * protect;
        (up * (1.0 - t) + base * t).try_normalize().unwrap_or(up)
    }
}

/// Per triangle, which edges are hard: shared with a triangle whose face normal is more than
/// about 30 degrees away. Edges are matched by position, so split vertices still pair up.
fn hard_edges(mesh: &Mesh) -> Vec<[bool; 3]> {
    let pt = |i: usize| mesh.positions.get(i).copied().unwrap_or(Vec3::ZERO);
    let key = |i: usize| mesh.positions.get(i).map_or([0; 3], |p| p.to_array().map(|c| (c + 0.0).to_bits()));
    let ordered = |a: [u32; 3], b: [u32; 3]| if a <= b { (a, b) } else { (b, a) };
    let count = mesh.triangle_count();
    let mut face = vec![Vec3::ZERO; count];
    let mut edges: HashMap<([u32; 3], [u32; 3]), Vec<usize>> = HashMap::new();
    for (t, f) in face.iter_mut().enumerate() {
        let Some(c) = mesh.tri(t) else { continue };
        *f = (pt(c[1]) - pt(c[0])).cross(pt(c[2]) - pt(c[0])).try_normalize().unwrap_or(Vec3::ZERO);
        for k in 0..3 {
            edges.entry(ordered(key(c[k]), key(c[(k + 1) % 3]))).or_default().push(t);
        }
    }
    (0..count)
        .map(|t| {
            let mut out = [false; 3];
            if let Some(c) = mesh.tri(t) {
                let own = face.get(t).copied().unwrap_or(Vec3::ZERO);
                for (k, hard) in out.iter_mut().enumerate() {
                    let others = edges.get(&ordered(key(c[k]), key(c[(k + 1) % 3])));
                    *hard = others.is_some_and(|ts| ts.iter().any(|&o| o != t && face.get(o).is_some_and(|f| f.dot(own) < HARD_EDGE_COS)));
                }
            }
            out
        })
        .collect()
}

/// Distance from `p` to the segment `a`-`b`.
fn segment_distance(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 1e-20 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + ab * t)).length()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold() -> Mesh {
        // Two triangles folded 90 degrees along (0,0,0)-(1,0,0), and a third continuing flat.
        Mesh { positions: vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(1.0, 1.0, 0.0)], indices: vec![0, 1, 2, 0, 3, 1, 1, 4, 2], ..Mesh::default() }
    }

    #[test]
    fn hard_edges_are_where_faces_meet_steeply() {
        let hard = hard_edges(&fold());
        assert!(hard[0][0] && hard[1][2], "the fold is hard from both sides: {hard:?}");
        assert!(!hard[0][1] && !hard[2][2], "the flat join is soft: {hard:?}");
        assert!((segment_distance(Vec3::new(0.5, 1.0, 0.0), Vec3::ZERO, Vec3::X) - 1.0).abs() < 1e-6);
        assert!(hard_edges(&Mesh::default()).is_empty());
    }

    #[test]
    fn maps_sample_bilinearly_and_bad_ones_are_refused() {
        let map = SkewMap { width: 2, height: 1, values: vec![0.0, 1.0], ..SkewMap::constant(0.0) };
        assert!(map.validate().is_ok());
        assert!(
            map.at(Vec2::new(0.0, 0.5)).abs() < 1e-6 && (map.at(Vec2::new(1.0, 0.5)) - 1.0).abs() < 1e-6 && (map.at(Vec2::new(0.5, 0.5)) - 0.5).abs() < 1e-6
        );
        assert!(map.at(Vec2::new(-3.0, 9.0)).is_finite() && map.at(Vec2::new(f32::NAN, 0.0)) == 0.0);
        for bad in [
            SkewMap { values: vec![0.0], ..map.clone() },
            SkewMap { width: 0, values: vec![], ..map.clone() },
            SkewMap { values: vec![0.0, f32::NAN], ..map.clone() },
            SkewMap { edge_distance: -1.0, ..map.clone() },
            SkewMap { edge_contrast: 0.0, ..map.clone() },
            SkewMap { width: u32::MAX, height: u32::MAX, ..map.clone() },
        ] {
            assert!(bad.validate().is_err());
        }
    }

    #[test]
    fn correction_blends_towards_the_base_and_edges_hold_it_back() {
        let mesh = fold();
        let up = Vec3::new(0.6, 0.8, 0.0);
        let base = Vec3::Y;
        let go = |map: &SkewMap, pos: Vec3| Prepared::new(map, &mesh, 1.0).unwrap().correct(&mesh, 0, pos, Vec2::splat(0.5), up, base, base);
        assert!((go(&SkewMap::constant(0.0), Vec3::ZERO) - up).length() < 1e-6, "black leaves the ray alone");
        assert!((go(&SkewMap::constant(1.0), Vec3::ZERO) - base).length() < 1e-6, "white turns it onto the base normal");
        let half = go(&SkewMap::constant(0.5), Vec3::ZERO);
        assert!(half.x > 0.0 && half.x < up.x && (half.length() - 1.0).abs() < 1e-5, "grey is in between and still unit: {half}");
        // Triangle 0's edge (0,0,0)-(1,0,0) is hard: on it the correction is held back, far from it it is not.
        let guarded = SkewMap { edge_distance: 0.5, ..SkewMap::constant(1.0) };
        assert!((go(&guarded, Vec3::new(0.5, 0.0, 0.0)) - up).length() < 1e-6, "on the hard edge the ray stays on the cage direction");
        assert!((go(&guarded, Vec3::new(0.3, 0.9, 0.0)) - base).length() < 1e-6, "away from every hard edge the correction is full");
    }

    #[test]
    fn painting_blends_dabs_in_and_resizing_keeps_the_picture() {
        let mut map = SkewMap::constant(0.0).resized(64, 64).unwrap();
        assert!(map.values.iter().all(|v| *v == 0.0));
        // One hard dab at the centre: white inside, untouched outside, in between at the rim.
        map.paint(&[Vec2::new(0.5, 0.5)], 0.2, 1.0, 1.0, 1.0).unwrap();
        let at = |m: &SkewMap, u: f32, v: f32| m.at(Vec2::new(u, v));
        assert!(at(&map, 0.5, 0.5) > 0.99 && at(&map, 0.95, 0.95) == 0.0, "{} {}", at(&map, 0.5, 0.5), at(&map, 0.95, 0.95));
        // Strength blends one dab in part of the way; a stroke's overlapping dabs build up towards the value; v is up.
        let mut dab = SkewMap::constant(0.0).resized(64, 64).unwrap();
        dab.paint(&[Vec2::new(0.5, 0.5)], 0.1, 0.8, 1.0, 0.5).unwrap();
        assert!((at(&dab, 0.5, 0.5) - 0.4).abs() < 0.02, "{}", at(&dab, 0.5, 0.5));
        let mut line = SkewMap::constant(0.0).resized(64, 64).unwrap();
        line.paint(&[Vec2::new(0.1, 0.9), Vec2::new(0.9, 0.9)], 0.05, 0.5, 1.0, 0.5).unwrap();
        assert!(at(&line, 0.5, 0.9) > 0.45 && at(&line, 0.5, 0.9) <= 0.5 + 1e-4, "{}", at(&line, 0.5, 0.9));
        assert!(at(&line, 0.5, 0.1) == 0.0, "the other end of v is untouched");
        // Erasing (value 0) brings a painted area back down.
        map.paint(&[Vec2::new(0.5, 0.5)], 0.2, 0.0, 1.0, 1.0).unwrap();
        assert!(at(&map, 0.5, 0.5) < 0.01);
        // Resampling keeps the picture and the edge settings.
        let mut small = SkewMap { edge_distance: 0.1, ..SkewMap::constant(0.0).resized(8, 8).unwrap() };
        small.paint(&[Vec2::new(0.25, 0.75)], 0.2, 1.0, 1.0, 1.0).unwrap();
        let big = small.resized(32, 32).unwrap();
        assert_eq!(big.edge_distance, 0.1);
        assert!((at(&big, 0.25, 0.75) - at(&small, 0.25, 0.75)).abs() < 0.1 && at(&big, 0.9, 0.1) == 0.0);
    }

    #[test]
    fn hostile_brushes_are_refused_and_change_nothing() {
        let mut map = SkewMap::constant(0.3).resized(16, 16).unwrap();
        let before = map.clone();
        let c = [Vec2::new(0.5, 0.5)];
        for (pts, r, v) in
            [(&c[..], 0.0, 1.0), (&c[..], -1.0, 1.0), (&c[..], f32::NAN, 1.0), (&c[..], 0.1, f32::INFINITY), (&[Vec2::new(f32::NAN, 0.0)][..], 0.1, 1.0)]
        {
            assert!(map.paint(pts, r, v, 0.5, 1.0).is_err(), "{pts:?} {r} {v}");
        }
        // Points far outside the atlas are harmless; a huge stroke is refused before any dab lands.
        map.paint(&[Vec2::new(1e6, -1e6)], 0.1, 1.0, 0.5, 1.0).unwrap();
        assert_eq!(map, before);
        assert!(map.paint(&[Vec2::ZERO, Vec2::new(1e6, 1e6)], 1e-4, 1.0, 0.5, 1.0).is_err());
        assert_eq!(map, before);
        // A big brush over a big map is refused up front, whatever the dab count: one dab here covers 16.7 M texels.
        let mut big = SkewMap::constant(0.0).resized(4096, 4096).unwrap();
        let untouched = big.clone();
        assert!(big.paint(&[Vec2::splat(0.5); 8], 1.0, 1.0, 0.5, 1.0).is_err());
        assert_eq!(big, untouched, "nothing was painted before the refusal");
        assert!(big.paint(&[Vec2::splat(0.5)], 1.0, 1.0, 0.5, 1.0).is_ok(), "one such dab is within the budget");
        assert!(SkewMap::constant(0.0).resized(0, 4).is_err() && SkewMap::constant(0.0).resized(100_000, 100_000).is_err());
    }
}
