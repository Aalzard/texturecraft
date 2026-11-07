//! Which polygons a polygon fill box selects.

use serde::{Deserialize, Serialize};
use texturecraft_geom::glam::Vec2;

/// How a polygon fill box decides whether it selects a polygon
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum FillHitTest {
    /// Any part of the polygon inside the box or crossing it
    #[default]
    Intersection,
    /// The polygon's center point inside the box
    ContainedCenter,
}

impl FillHitTest {
    /// Whether the box from `low` to `high` selects the polygon made of `triangles` (their corners)
    pub fn selects(self, triangles: &[[Vec2; 3]], low: Vec2, high: Vec2) -> bool {
        match self {
            FillHitTest::Intersection => triangles.iter().any(|corners| triangle_overlaps_box(*corners, low, high)),
            FillHitTest::ContainedCenter => center(triangles).is_some_and(|point| point.cmpge(low).all() && point.cmple(high).all()),
        }
    }
}

/// Area-weighted center of the triangles (their corners' mean when they have no area)
fn center(triangles: &[[Vec2; 3]]) -> Option<Vec2> {
    let area = |corners: &[Vec2; 3]| (corners[1] - corners[0]).perp_dot(corners[2] - corners[0]).abs();
    let centroid = |corners: &[Vec2; 3]| (corners[0] + corners[1] + corners[2]) / 3.0;
    let total: f32 = triangles.iter().map(area).sum();
    let weighted: Vec2 = triangles.iter().map(|corners| centroid(corners) * area(corners)).sum();
    let centroids: Vec2 = triangles.iter().map(centroid).sum();

    match triangles.len() {
        0 => None,
        count if total <= f32::EPSILON => Some(centroids / count as f32),
        _ => Some(weighted / total),
    }
}

/// Separating axis test of a triangle against an axis-aligned box
fn triangle_overlaps_box(corners: [Vec2; 3], low: Vec2, high: Vec2) -> bool {
    let triangle_low = corners[0].min(corners[1]).min(corners[2]);
    let triangle_high = corners[0].max(corners[1]).max(corners[2]);
    if triangle_high.cmplt(low).any() || triangle_low.cmpgt(high).any() {
        return false;
    }

    let box_corners = [low, Vec2::new(high.x, low.y), high, Vec2::new(low.x, high.y)];
    (0..3).all(|edge| {
        let axis = (corners[(edge + 1) % 3] - corners[edge]).perp();
        let span = |points: &[Vec2]| points.iter().fold((f32::MAX, f32::MIN), |(min, max), point| (min.min(axis.dot(*point)), max.max(axis.dot(*point))));
        let (triangle_min, triangle_max) = span(&corners);
        let (box_min, box_max) = span(&box_corners);
        triangle_max >= box_min && box_max >= triangle_min
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRIANGLE: [Vec2; 3] = [Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(0.0, 4.0)];

    #[test]
    fn intersection_needs_any_overlap_and_contained_center_the_center() {
        let (low, high) = (Vec2::new(2.0, 2.0), Vec2::new(6.0, 6.0));
        assert!(FillHitTest::Intersection.selects(&[TRIANGLE], Vec2::new(1.0, 1.0), high), "a box cutting the triangle");
        assert!(!FillHitTest::Intersection.selects(&[TRIANGLE], Vec2::new(3.0, 3.0), Vec2::new(5.0, 5.0)), "inside the bounds, past the long edge");
        assert!(!FillHitTest::ContainedCenter.selects(&[TRIANGLE], low, high), "the center (4/3, 4/3) lies outside");
        assert!(FillHitTest::ContainedCenter.selects(&[TRIANGLE], Vec2::splat(1.0), Vec2::splat(2.0)));
    }

    #[test]
    fn a_polygon_center_weighs_its_triangles_by_area() {
        let square = [[Vec2::ZERO, Vec2::new(2.0, 0.0), Vec2::new(2.0, 2.0)], [Vec2::ZERO, Vec2::new(2.0, 2.0), Vec2::new(0.0, 2.0)]];
        assert_eq!(center(&square), Some(Vec2::ONE));
        assert_eq!(center(&[]), None);
    }
}
