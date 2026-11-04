//! Symmetry guides: where the mirror planes (and radial copy planes) cut the mesh surface.
//!
//! The planes pass through [`symmetry_center`], the same point [`crate::stroke::Stroke`] mirrors
//! stamps about, so a guide line is exactly where a mirrored dab meets its original. Sections are
//! plain CPU geometry from the triangles; the viewport draws them as an overlay.

use texturecraft_geom::Mesh;
use texturecraft_geom::glam::Vec3;

use crate::brush::Symmetry;

/// Most guide segments produced for one mesh (a hostile or huge mesh cannot flood the GPU).
pub const MAX_SEGMENTS: usize = 400_000;

/// The point every mirror and radial copy is made about: the center of the mesh bounds.
pub fn symmetry_center(mesh: &Mesh) -> Vec3 {
    let (lo, hi) = mesh.bounds();
    (lo + hi) * 0.5
}

/// Which plane a guide segment belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuideKind {
    /// A mirror plane across the axis (0 = X, 1 = Y, 2 = Z).
    Mirror(u8),
    /// A boundary between two radial copies.
    Radial,
}

/// One piece of a section line, in mesh space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuideSegment {
    pub a: Vec3,
    pub b: Vec3,
    pub kind: GuideKind,
}

fn unit_axis(axis: u8) -> Vec3 {
    [Vec3::X, Vec3::Y, Vec3::Z].get(usize::from(axis)).copied().unwrap_or(Vec3::Y)
}

/// The segments where the plane through `origin` with `normal` cuts the triangles of `mesh`.
/// Triangles lying in the plane and plane touches at a single point give nothing; an edge
/// lying in the plane gives one segment. Stops at `limit` segments.
pub fn section_plane(mesh: &Mesh, origin: Vec3, normal: Vec3, limit: usize) -> Vec<(Vec3, Vec3)> {
    let n = normal.normalize_or_zero();
    let mut out = Vec::new();
    // Edges lying in the plane belong to two triangles: draw them once.
    let mut in_plane = std::collections::HashSet::new();
    if n == Vec3::ZERO || !origin.is_finite() {
        return out;
    }
    let (lo, hi) = mesh.bounds();
    // Distances this small count as "on the plane" (keeps vertices exactly on it from splitting).
    let eps = (hi - lo).length().max(1e-6) * 1e-6;
    for tri in mesh.indices.chunks_exact(3) {
        let (Some(&i0), Some(&i1), Some(&i2)) = (tri.first(), tri.get(1), tri.get(2)) else { continue };
        let (Some(&p0), Some(&p1), Some(&p2)) = (mesh.positions.get(i0 as usize), mesh.positions.get(i1 as usize), mesh.positions.get(i2 as usize)) else {
            continue;
        };
        let p = [p0, p1, p2];
        let d = p.map(|q| {
            let x = (q - origin).dot(n);
            if x.abs() <= eps { 0.0 } else { x }
        });
        if d.iter().all(|x| *x > 0.0) || d.iter().all(|x| *x < 0.0) || d.iter().all(|x| *x == 0.0) {
            continue;
        }
        let mut pts: Vec<Vec3> = Vec::with_capacity(3);
        for k in 0..3 {
            let (j, dk, dj) = ((k + 1) % 3, d[k], d[(k + 1) % 3]);
            if dk == 0.0 {
                pts.push(p[k]);
            } else if dk * dj < 0.0 {
                pts.push(p[k].lerp(p[j], dk / (dk - dj)));
            }
        }
        if let [a, b] = pts[..] {
            if d.iter().filter(|x| **x == 0.0).count() == 2 {
                let mut ends = tri.iter().zip(d).filter(|(_, x)| *x == 0.0).map(|(i, _)| *i);
                if let (Some(e0), Some(e1)) = (ends.next(), ends.next())
                    && !in_plane.insert((e0.min(e1), e0.max(e1)))
                {
                    continue;
                }
            }
            out.push((a, b));
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

/// Keep the part of segment `a`–`b` with `(x - origin) · dir >= 0`.
fn clip_half(a: Vec3, b: Vec3, origin: Vec3, dir: Vec3) -> Option<(Vec3, Vec3)> {
    let (da, db) = ((a - origin).dot(dir), (b - origin).dot(dir));
    match (da >= 0.0, db >= 0.0) {
        (true, true) => Some((a, b)),
        (false, false) => None,
        (true, false) => Some((a, a.lerp(b, da / (da - db)))),
        (false, true) => Some((a.lerp(b, da / (da - db)), b)),
    }
}

/// Section lines for every plane the brush symmetry uses: one per enabled mirror axis, and for
/// radial symmetry (`radial` ≥ 2 copies) the half-planes through the radial axis that separate
/// the copies. All pass through [`symmetry_center`].
pub fn symmetry_guides(mesh: &Mesh, sym: &Symmetry) -> Vec<GuideSegment> {
    let mut out = Vec::new();
    if !sym.any() {
        return out;
    }
    let center = symmetry_center(mesh);
    for (axis, on) in [(0u8, sym.x), (1, sym.y), (2, sym.z)] {
        if on {
            let room = MAX_SEGMENTS.saturating_sub(out.len());
            out.extend(section_plane(mesh, center, unit_axis(axis), room).into_iter().map(|(a, b)| GuideSegment { a, b, kind: GuideKind::Mirror(axis) }));
        }
    }
    if sym.radial > 1 {
        let axis = unit_axis(sym.axis.min(2));
        // Two directions perpendicular to the axis to measure the copy angle from.
        let u = unit_axis((sym.axis.min(2) + 1) % 3);
        let v = axis.cross(u);
        let copies = f32::from(sym.radial.min(32));
        for k in 0..sym.radial.min(32) {
            // Copy k sits at k/N of a turn; the boundary to its neighbour is half a step on.
            let angle = (f32::from(k) + 0.5) * std::f32::consts::TAU / copies;
            let dir = u * angle.cos() + v * angle.sin();
            let room = MAX_SEGMENTS.saturating_sub(out.len());
            for (a, b) in section_plane(mesh, center, axis.cross(dir), room) {
                if let Some((a, b)) = clip_half(a, b, center, dir) {
                    out.push(GuideSegment { a, b, kind: GuideKind::Radial });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use texturecraft_geom::primitives;

    /// A closed box 2 wide (x), 4 tall (y), 6 deep (z), offset so its center is not the origin.
    fn offset_box() -> Mesh {
        let mut m = primitives::cube(1.0);
        let (lo, hi) = m.bounds();
        for p in &mut m.positions {
            *p = (*p - (lo + hi) * 0.5) / (hi - lo) * Vec3::new(2.0, 4.0, 6.0) + Vec3::new(10.0, -3.0, 5.0);
        }
        m
    }

    #[test]
    fn center_is_the_bounds_center_the_strokes_mirror_about() {
        let m = offset_box();
        let c = symmetry_center(&m);
        assert!((c - Vec3::new(10.0, -3.0, 5.0)).length() < 1e-4, "{c}");
        let stroke = crate::stroke::Stroke::new(&crate::BrushSettings::default(), &m);
        assert_eq!(stroke.mirror_center(), c);
    }

    #[test]
    fn section_of_a_box_is_a_loop_in_the_plane() {
        let m = offset_box();
        let c = symmetry_center(&m);
        for axis in 0..3u8 {
            let n = unit_axis(axis);
            let segs = section_plane(&m, c, n, usize::MAX);
            // A box cut through its middle: a rectangle outline (4 sides), each side crossing
            // two triangles' worth of the box faces, at least 4 segments.
            assert!(segs.len() >= 4, "axis {axis}: {}", segs.len());
            for (a, b) in &segs {
                assert!((a - c).dot(n).abs() < 1e-3 && (b - c).dot(n).abs() < 1e-3, "axis {axis}: not on the plane");
                assert!(a.distance(*b) > 0.0);
            }
        }
        // The outline of the X cut is the 4x6 rectangle (y by z): total length 2*(4+6).
        let len: f32 = section_plane(&m, c, Vec3::X, usize::MAX).iter().map(|(a, b)| a.distance(*b)).sum();
        assert!((len - 20.0).abs() < 1e-2, "{len}");
    }

    #[test]
    fn plane_outside_the_mesh_or_degenerate_gives_nothing() {
        let m = offset_box();
        assert!(section_plane(&m, Vec3::new(100.0, 0.0, 0.0), Vec3::X, usize::MAX).is_empty());
        assert!(section_plane(&m, Vec3::ZERO, Vec3::ZERO, usize::MAX).is_empty());
        assert!(section_plane(&m, Vec3::splat(f32::NAN), Vec3::X, usize::MAX).is_empty());
        assert!(section_plane(&Mesh::default(), Vec3::ZERO, Vec3::X, usize::MAX).is_empty());
        // Truncated index list and out-of-range indices are skipped, not a panic.
        let mut bad = m.clone();
        bad.indices.push(99_999);
        bad.indices.extend([0, 99_999, 1]);
        let _ = section_plane(&bad, symmetry_center(&bad), Vec3::X, usize::MAX);
        assert_eq!(section_plane(&m, symmetry_center(&m), Vec3::X, 3).len(), 3);
    }

    #[test]
    fn guides_follow_the_enabled_planes_only() {
        let m = offset_box();
        let c = symmetry_center(&m);
        assert!(symmetry_guides(&m, &Symmetry::default()).is_empty());
        let sym = Symmetry { x: true, z: true, ..Symmetry::default() };
        let g = symmetry_guides(&m, &sym);
        assert!(g.iter().all(|s| matches!(s.kind, GuideKind::Mirror(0) | GuideKind::Mirror(2))));
        assert!(g.iter().any(|s| s.kind == GuideKind::Mirror(0)) && g.iter().any(|s| s.kind == GuideKind::Mirror(2)));
        for s in &g {
            let GuideKind::Mirror(axis) = s.kind else { continue };
            let n = unit_axis(axis);
            assert!((s.a - c).dot(n).abs() < 1e-3 && (s.b - c).dot(n).abs() < 1e-3);
        }
    }

    #[test]
    fn radial_guides_are_half_planes_between_the_copies() {
        let m = primitives::sphere(1.0, 64, 32);
        let c = symmetry_center(&m);
        let sym = Symmetry { radial: 4, axis: 1, ..Symmetry::default() };
        let g = symmetry_guides(&m, &sym);
        assert!(g.iter().all(|s| s.kind == GuideKind::Radial) && !g.is_empty());
        // Every point lies on one of the four half-planes at 45 + 90k degrees around Y.
        for s in &g {
            for p in [s.a, s.b] {
                let r = p - c;
                if r.x.hypot(r.z) < 1e-3 {
                    continue; // on the axis itself
                }
                let deg = r.z.atan2(r.x).to_degrees().rem_euclid(90.0);
                assert!((deg - 45.0).abs() < 0.5, "{deg}");
            }
        }
        // Radial copies: 2 gives a single plane's two halves; a lone copy gives no guide.
        assert!(!symmetry_guides(&m, &Symmetry { radial: 2, ..Symmetry::default() }).is_empty());
        assert!(symmetry_guides(&m, &Symmetry { radial: 1, ..Symmetry::default() }).is_empty());
    }
}
