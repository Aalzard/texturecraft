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
}
