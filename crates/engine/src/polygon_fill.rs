//! Polygon fill: fill the triangles, polygons, UV islands or connected meshes under a click or inside a dragged box, in either view.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashSet};

use texturecraft_geom::glam::{Vec2, Vec4Swizzles};
use texturecraft_geom::{Mesh, NO_TRI};
use texturecraft_paint::{EyeDepth, fill_triangles};

use crate::polygon_fill_drag::PolygonFillDrag;
use crate::session::{FillMode, Session};
use crate::triangle_partition::TrianglePartition;
use crate::{EngineError, Result};

/// Corners closer than one step of this grid (per unit of position or UV) count as the same corner
const CORNER_GRID_STEPS: f32 = 1e5;

/// Clip-space w below which a vertex counts as behind the camera
const MIN_CLIP_W: f32 = 1e-5;

impl Session {
    /// Fill the triangle / polygon / UV island / connected mesh under viewport pixel `screen`
    pub fn polygon_fill(&mut self, screen: Vec2, viewport: Vec2) -> Result<usize> {
        let hit = self.pick(screen, viewport).ok_or_else(|| EngineError::Other("nothing under the cursor".into()))?;
        if hit.material != self.fill_material()? {
            return Err(EngineError::Other("that surface belongs to another texture set".into()));
        }

        self.fill_group_of(hit.tri)
    }

    /// Fill like [`Session::polygon_fill`] at texture coordinate `uv` (the 2D view)
    pub fn polygon_fill_uv(&mut self, uv: Vec2) -> Result<usize> {
        let material = self.fill_material()?;
        let mesh = &self.mesh;
        let triangle = mesh
            .triangles_of(material)
            .find(|triangle| uv_corners(mesh, *triangle).is_some_and(|corners| contains(corners, uv)))
            .ok_or_else(|| EngineError::Other("nothing under the cursor".into()))?;

        self.fill_group_of(triangle as u32)
    }

    /// Fill what the box between viewport pixels `corner` and `opposite` selects (one undo step)
    pub fn polygon_fill_box(&mut self, corner: Vec2, opposite: Vec2, viewport: Vec2) -> Result<usize> {
        self.drag_polygon_fill_box(corner, opposite, viewport)?;
        Ok(self.end_polygon_fill_box())
    }

    /// Fill what the box between texture coordinates `corner` and `opposite` selects (the 2D view, one undo step)
    pub fn polygon_fill_box_uv(&mut self, corner: Vec2, opposite: Vec2) -> Result<usize> {
        self.drag_polygon_fill_box_uv(corner, opposite)?;
        Ok(self.end_polygon_fill_box())
    }

    /// Show the fill of a box being dragged between viewport pixels `corner` and `opposite`
    pub fn drag_polygon_fill_box(&mut self, corner: Vec2, opposite: Vec2, viewport: Vec2) -> Result<usize> {
        let (low, high) = box_bounds(corner, opposite)?;
        let viewport = viewport.max(Vec2::ONE);
        self.start_polygon_fill_drag(Some(viewport))?;

        let view_proj = self.camera.view_proj(viewport.x / viewport.y);
        let mesh = self.mesh.clone();
        let to_screen = |vertex: usize| {
            let clip = view_proj * mesh.positions.get(vertex)?.extend(1.0);
            let ndc = (clip.w > MIN_CLIP_W).then(|| clip.xy() / clip.w)?;
            Some(Vec2::new(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * viewport)
        };
        let screen_corners = |triangle: usize| {
            let [first, second, third] = mesh.tri(triangle)?;
            Some([to_screen(first)?, to_screen(second)?, to_screen(third)?])
        };
        let selected = self.box_selection(&screen_corners, low, high);
        self.show_box_fill(selected)
    }

    /// Show the fill of a box being dragged between texture coordinates `corner` and `opposite` (the 2D view)
    pub fn drag_polygon_fill_box_uv(&mut self, corner: Vec2, opposite: Vec2) -> Result<usize> {
        let (low, high) = box_bounds(corner, opposite)?;
        self.start_polygon_fill_drag(None)?;

        let mesh = self.mesh.clone();
        let selected = self.box_selection(&|triangle| uv_corners(&mesh, triangle), low, high);
        self.show_box_fill(selected)
    }

    /// Finish the dragged box: what it filled becomes one undo step. Returns the filled triangle count
    pub fn end_polygon_fill_box(&mut self) -> usize {
        let Some(drag) = self.polygon_fill_drag.take() else { return 0 };

        // Quick mask edits keep no history, as with its strokes
        let filled = drag.filled.len();
        if filled > 0 && drag.quick_mask_before.is_none() {
            self.push_history("Polygon fill", drag.before);
        }
        filled
    }

    /// The mesh material of the texture set being painted
    fn fill_material(&self) -> Result<usize> {
        Ok(self.doc.project.texture_set(self.ts())?.material)
    }

    /// The triangles the current fill mode fills together, for `material`
    fn fill_partition(&self, material: usize) -> TrianglePartition {
        match self.tool.fill_mode {
            FillMode::Triangle => TrianglePartition::new(&self.mesh, material, |_, _| Vec::<u32>::new()),
            FillMode::Polygon => TrianglePartition::new(&self.mesh, material, polygon_edges),
            FillMode::UvIsland => TrianglePartition::new(&self.mesh, material, uv_edges),
            FillMode::Mesh => TrianglePartition::new(&self.mesh, material, position_edges),
        }
    }

    /// Fill the fill-mode group holding `triangle` (one undo step)
    fn fill_group_of(&mut self, triangle: u32) -> Result<usize> {
        let ts = self.ts();
        let groups = self.fill_partition(self.fill_material()?);
        let triangles: BTreeSet<u32> = groups.group(triangle).unwrap_or_default().iter().copied().collect();

        let (gbuf, mut canvas) = self.fill_canvas(ts)?;
        fill_triangles(&mut canvas, &gbuf, &triangles);
        self.commit_fill("Polygon fill", ts, &canvas)?;
        Ok(triangles.len())
    }

    /// Begin a box drag unless one is under way; `viewport` is the 3D view's size (None in the 2D view)
    fn start_polygon_fill_drag(&mut self, viewport: Option<Vec2>) -> Result<()> {
        if self.polygon_fill_drag.is_some() {
            return Ok(());
        }

        let material = self.fill_material()?;
        let polygons = match self.tool.fill_mode {
            FillMode::Triangle => TrianglePartition::new(&self.mesh, material, |_, _| Vec::<u32>::new()),
            _ => TrianglePartition::new(&self.mesh, material, polygon_edges),
        };
        let depth = viewport
            .filter(|_| self.tool.fill_visible_only)
            .map(|size| (EyeDepth::render(&self.shown_mesh(), &self.camera, size.x as u32, size.y as u32), size));
        self.polygon_fill_drag = Some(PolygonFillDrag {
            before: self.doc.clone(),
            quick_mask_before: self.quick_mask.editing.then(|| self.quick_mask.masks.clone()),
            filled: BTreeSet::new(),
            depth,
            polygons,
            fill_groups: self.fill_partition(material),
            failed: false,
        });
        Ok(())
    }

    /// The mesh as the 3D view shows it: hidden texture sets' triangles collapse to a point, so nothing behind them counts as hidden
    fn shown_mesh(&self) -> Cow<'_, Mesh> {
        let sets = &self.doc.project.texture_sets;
        if sets.iter().all(|set| set.visible) {
            return Cow::Borrowed(&self.mesh);
        }

        let mut mesh = (*self.mesh).clone();
        for set in sets.iter().filter(|set| !set.visible) {
            for triangle in self.mesh.triangles_of(set.material) {
                if let Some(corners) = mesh.indices.get_mut(triangle * 3..triangle * 3 + 3) {
                    let first = corners[0];
                    corners.fill(first);
                }
            }
        }
        Cow::Owned(mesh)
    }

    /// Triangles the dragged box from `low` to `high` fills, given each triangle's `corners` in the box's space
    fn box_selection(&self, corners: &dyn Fn(usize) -> Option<[Vec2; 3]>, low: Vec2, high: Vec2) -> BTreeSet<u32> {
        let Some(drag) = &self.polygon_fill_drag else { return BTreeSet::new() };

        let visible = drag.depth.as_ref().map(|(depth, viewport)| visible_triangles(depth, *viewport, low, high));
        let hit_test = self.tool.fill_hit_test;
        let mut groups = BTreeSet::new();
        for polygon in &drag.polygons.groups {
            let seen = visible.as_ref().is_none_or(|visible| polygon.iter().any(|triangle| visible.contains(triangle)));
            let triangles: Option<Vec<[Vec2; 3]>> = polygon.iter().map(|triangle| corners(*triangle as usize)).collect();
            if seen && triangles.is_some_and(|triangles| hit_test.selects(&triangles, low, high)) {
                groups.extend(polygon.iter().filter_map(|triangle| drag.fill_groups.index_of(*triangle)));
            }
        }

        groups.iter().filter_map(|group| drag.fill_groups.groups.get(*group)).flatten().copied().collect()
    }

    /// Show the box drag's fill of `selected`: the document before the drag, refilled when the selection changed
    fn show_box_fill(&mut self, selected: BTreeSet<u32>) -> Result<usize> {
        let Some(drag) = self.polygon_fill_drag.as_mut() else { return Ok(0) };
        if drag.failed || drag.filled == selected {
            return Ok(drag.filled.len());
        }

        drag.filled = selected.clone();
        let previous = std::mem::replace(&mut self.doc, drag.before.clone());
        if let Some(masks) = &drag.quick_mask_before {
            self.quick_mask.masks = masks.clone();
        }

        let ts = self.ts();
        let painted = if selected.is_empty() {
            Ok(())
        } else {
            self.fill_canvas(ts).and_then(|(gbuf, mut canvas)| {
                fill_triangles(&mut canvas, &gbuf, &selected);
                self.paint_fill(ts, &canvas)
            })
        };
        self.invalidate_changed(&previous);
        self.quick_mask.revision = self.quick_mask.revision.wrapping_add(1);

        // A failed fill is reported once; the rest of the drag fills nothing
        if painted.is_err()
            && let Some(mut drag) = self.polygon_fill_drag.take()
        {
            self.restore_before_box(&drag);
            drag.failed = true;
            drag.filled.clear();
            self.polygon_fill_drag = Some(drag);
        }
        painted.map(|_| selected.len())
    }

    /// Put back the document and quick mask from before the box `drag`
    fn restore_before_box(&mut self, drag: &PolygonFillDrag) {
        let previous = std::mem::replace(&mut self.doc, drag.before.clone());
        if let Some(masks) = &drag.quick_mask_before {
            self.quick_mask.masks = masks.clone();
            self.quick_mask.revision = self.quick_mask.revision.wrapping_add(1);
        }
        self.invalidate_changed(&previous);
    }
}

/// Triangles `depth` (rendered for `viewport`) shows in the viewport pixels from `low` to `high`
fn visible_triangles(depth: &EyeDepth, viewport: Vec2, low: Vec2, high: Vec2) -> HashSet<u32> {
    let pixels_per_point = Vec2::new(depth.width as f32, depth.height as f32) / viewport;
    let (first, last) = ((low * pixels_per_point).floor(), (high * pixels_per_point).ceil());
    let columns = first.x.max(0.0) as usize..(last.x as usize).min(depth.width as usize);
    let rows = first.y.max(0.0) as usize..(last.y as usize).min(depth.height as usize);

    rows.flat_map(|row| depth.triangle.get(row * depth.width as usize..).into_iter().flat_map(|pixels| pixels.iter().take(columns.end).skip(columns.start)))
        .copied()
        .filter(|triangle| *triangle != NO_TRI)
        .collect()
}

/// The two corners spanning a box, lowest first
fn box_bounds(corner: Vec2, opposite: Vec2) -> Result<(Vec2, Vec2)> {
    if !corner.is_finite() || !opposite.is_finite() {
        return Err(EngineError::Other("the box corners must be finite".into()));
    }

    Ok((corner.min(opposite), corner.max(opposite)))
}

/// Texture coordinates of `triangle`'s corners
fn uv_corners(mesh: &Mesh, triangle: usize) -> Option<[Vec2; 3]> {
    let [first, second, third] = mesh.tri(triangle)?;
    Some([*mesh.uvs.get(first)?, *mesh.uvs.get(second)?, *mesh.uvs.get(third)?])
}

/// Whether `point` lies in the triangle with `corners` (either winding)
fn contains(corners: [Vec2; 3], point: Vec2) -> bool {
    let side = |edge: usize| (corners[(edge + 1) % 3] - corners[edge]).perp_dot(point - corners[edge]);
    let sides = [side(0), side(1), side(2)];
    sides.iter().all(|side| *side >= 0.0) || sides.iter().all(|side| *side <= 0.0)
}

/// The polygon diagonals of `triangle`: edges the importer added to split a polygon
fn polygon_edges(mesh: &Mesh, triangle: usize) -> Vec<(u32, u32)> {
    let hidden = mesh.diagonals.get(triangle).copied().unwrap_or(0);
    let Some(vertices) = mesh.indices.get(triangle * 3..triangle * 3 + 3) else { return Vec::new() };

    (0..3)
        .filter(|edge| hidden >> edge & 1 == 1)
        .map(|edge| {
            let (from, to) = (vertices[edge], vertices[(edge + 1) % 3]);
            (from.min(to), from.max(to))
        })
        .collect()
}

/// `triangle`'s edges in UV space: imports often split vertices along hard edges where the UVs run on, so vertex indices alone break islands apart
fn uv_edges(mesh: &Mesh, triangle: usize) -> Vec<([i64; 2], [i64; 2])> {
    let corner = |uv: Vec2| (uv * CORNER_GRID_STEPS).round().to_array().map(|step| step as i64);
    let Some([first, second, third]) = uv_corners(mesh, triangle).map(|uvs| uvs.map(corner)) else { return Vec::new() };

    [(first, second), (second, third), (third, first)].map(|(from, to)| (from.min(to), from.max(to))).to_vec()
}

/// `triangle`'s edges by position: UV seams and hard edges split vertices but leave the geometry connected
fn position_edges(mesh: &Mesh, triangle: usize) -> Vec<([i64; 3], [i64; 3])> {
    let corner = |vertex: usize| mesh.positions.get(vertex).map(|position| (*position * CORNER_GRID_STEPS).round().to_array().map(|step| step as i64));
    let Some([first, second, third]) = mesh.tri(triangle) else { return Vec::new() };
    let (Some(first), Some(second), Some(third)) = (corner(first), corner(second), corner(third)) else { return Vec::new() };

    [(first, second), (second, third), (third, first)].map(|(from, to)| (from.min(to), from.max(to))).to_vec()
}

#[cfg(test)]
mod tests {
    use texturecraft_core::Channel;
    use texturecraft_geom::glam::{Vec2, Vec3};
    use texturecraft_geom::mesh::Mesh;
    use texturecraft_geom::primitives;

    use crate::fill_hit_test::FillHitTest;
    use crate::session::{FillMode, Session, Tool};

    /// The roughness the test fills paint, to tell filled texels apart
    const FILL_ROUGHNESS: f32 = 0.9;

    /// A polygon fill session on `mesh` painting roughness [`FILL_ROUGHNESS`]
    fn fill_session(mesh: Mesh) -> Session {
        let mut session = Session::new_project("t", mesh, "test", None, 128).unwrap();
        session.viewport = Vec2::new(400.0, 400.0);
        session.tool.tool = Tool::PolygonFill;
        session.tool.values.insert(Channel::Roughness, [FILL_ROUGHNESS; 3]);
        session.update();
        session
    }

    /// Whether the texel at texture coordinate `uv` shows the fill's roughness
    fn filled_at(session: &Session, uv: Vec2) -> bool {
        let (width, height) = session.caches[0].size;
        let roughness = session.caches[0].composite.value(Channel::Roughness, (uv.x * width as f32) as u32, (uv.y * height as f32) as u32);
        (roughness[0] - FILL_ROUGHNESS).abs() < 0.01
    }

    /// Two copies of a plane apart in space and in UV space, each triangle with its own three vertices (as hard edges or per-face imports leave a mesh)
    fn split_planes() -> (Mesh, usize) {
        let plane = primitives::plane(2.0, 4);
        let (mut positions, mut uvs) = (Vec::<Vec3>::new(), Vec::<Vec2>::new());
        for shift in [0.0, 0.5] {
            for vertex in plane.indices.iter().map(|index| *index as usize) {
                positions.push(plane.positions[vertex] + Vec3::Y * shift);
                uvs.push(plane.uvs[vertex] * 0.5 + shift);
            }
        }
        let indices = (0..positions.len() as u32).collect();
        let mesh = Mesh::from_parts(positions, None, uvs, indices, "m").unwrap();
        assert_eq!(mesh.vertex_count(), 6 * plane.triangle_count(), "no vertex is shared");
        (mesh, plane.triangle_count())
    }

    #[test]
    fn uv_island_and_mesh_fills_follow_seams_split_vertices_leave_apart() {
        let (mesh, per_plane) = split_planes();
        let mut session = Session::new_project("t", mesh, "test", None, 128).unwrap();

        for fill_mode in [FillMode::UvIsland, FillMode::Mesh] {
            session.tool.fill_mode = fill_mode;
            let groups = session.fill_partition(0);
            assert_eq!(groups.group(0).map(<[u32]>::len), Some(per_plane), "{fill_mode:?}: the whole first plane");
            assert_eq!(groups.group(per_plane as u32).map(<[u32]>::len), Some(per_plane), "{fill_mode:?}: only the second plane");
        }
    }

    #[test]
    fn polygon_mode_groups_the_two_triangles_of_each_quad() {
        let mut session = fill_session(primitives::plane(2.0, 2));

        session.tool.fill_mode = FillMode::Triangle;
        assert_eq!(session.fill_partition(0).group(0), Some(&[0][..]));

        session.tool.fill_mode = FillMode::Polygon;
        let mut quad = session.fill_partition(0).group(0).unwrap().to_vec();
        quad.sort_unstable();
        assert_eq!(quad, [0, 1], "the quad's two triangles and no other quad");
    }

    #[test]
    fn a_uv_box_over_half_a_plane_fills_that_half() {
        let mut session = fill_session(primitives::plane(2.0, 4));
        session.tool.fill_hit_test = FillHitTest::ContainedCenter;

        let filled = session.polygon_fill_box_uv(Vec2::new(-0.01, -0.01), Vec2::new(0.5, 1.01)).unwrap();
        session.update();

        assert_eq!(filled, 16, "the left two columns of quads");
        assert!(filled_at(&session, Vec2::new(0.1, 0.5)) && filled_at(&session, Vec2::new(0.45, 0.9)));
        assert!(!filled_at(&session, Vec2::new(0.55, 0.5)) && !filled_at(&session, Vec2::new(0.9, 0.1)));
    }

    #[test]
    fn intersection_and_contained_center_differ_on_a_box_missing_a_quads_center() {
        let mut session = fill_session(primitives::plane(2.0, 2));
        session.tool.fill_mode = FillMode::Polygon;
        let (corner, opposite) = (Vec2::new(0.0, 0.0), Vec2::new(0.2, 0.2));

        session.tool.fill_hit_test = FillHitTest::ContainedCenter;
        assert_eq!(session.polygon_fill_box_uv(corner, opposite).unwrap(), 0, "the quad's center (0.25, 0.25) is outside");
        assert_ne!(session.undo_label(), Some("Polygon fill"), "an empty box records no undo step");

        session.tool.fill_hit_test = FillHitTest::Intersection;
        assert_eq!(session.polygon_fill_box_uv(corner, opposite).unwrap(), 2, "the quad the box cuts");
    }

    #[test]
    fn visible_only_skips_the_back_of_a_cube() {
        let mut session = fill_session(primitives::cube(1.0));
        session.camera.yaw = 0.6;
        session.camera.pitch = 0.4;
        let (low, high) = (Vec2::ZERO, session.viewport);
        let every_triangle = session.mesh.triangle_count();

        session.tool.fill_visible_only = true;
        assert_eq!(session.polygon_fill_box(low, high, session.viewport).unwrap(), every_triangle / 2, "the three sides the camera sees");

        session.tool.fill_visible_only = false;
        assert_eq!(session.polygon_fill_box(low, high, session.viewport).unwrap(), every_triangle);
    }

    #[test]
    fn visible_only_sees_through_a_hidden_texture_set() {
        let mut session = fill_session(primitives::material_ball());
        let base = session.doc.project.texture_sets.iter().position(|set| set.name == "Base").unwrap();
        session.active_set = base;
        let (low, high) = (Vec2::ZERO, session.viewport);

        let behind_the_ball = session.polygon_fill_box(low, high, session.viewport).unwrap();
        session.doc.project.texture_sets[0].visible = false;
        let ball_hidden = session.polygon_fill_box(low, high, session.viewport).unwrap();

        assert!(ball_hidden > behind_the_ball, "hiding the ball shows more of the base: {ball_hidden} vs {behind_the_ball}");
    }

    #[test]
    fn a_dragged_box_fills_live_and_records_one_step_when_it_ends() {
        let mut session = fill_session(primitives::plane(2.0, 4));
        let history = session.history_labels().len();

        assert_eq!(session.drag_polygon_fill_box_uv(Vec2::ZERO, Vec2::ONE).unwrap(), 32);
        assert_eq!(session.drag_polygon_fill_box_uv(Vec2::ZERO, Vec2::new(0.45, 1.0)).unwrap(), 16, "shrinking the box unfills");
        session.update();
        assert!(filled_at(&session, Vec2::new(0.2, 0.5)) && !filled_at(&session, Vec2::new(0.8, 0.5)), "the fill shows while dragging");
        assert_eq!(session.history_labels().len(), history, "no undo step yet");

        assert_eq!(session.end_polygon_fill_box(), 16);
        assert_eq!(session.history_labels().len(), history + 1);
        session.undo().unwrap();
        session.update();
        assert!(!filled_at(&session, Vec2::new(0.2, 0.5)), "one undo removes the whole drag");
    }

    #[test]
    fn a_uv_box_over_the_second_uv_tile_fills_that_tile_only() {
        let mut session = fill_session(primitives::udim_plane());
        assert_eq!(session.doc.project.texture_sets[0].uv_tiles, [1001, 1002]);
        session.tool.fill_hit_test = FillHitTest::ContainedCenter;

        let every_triangle = session.mesh.triangle_count();
        assert_eq!(session.polygon_fill_box_uv(Vec2::new(0.5, 0.0), Vec2::ONE).unwrap(), every_triangle / 2, "the atlas's right half is tile 1002");
    }

    #[test]
    fn a_2d_click_fills_the_triangle_under_it() {
        let mut session = fill_session(primitives::plane(2.0, 1));

        assert_eq!(session.polygon_fill_uv(Vec2::new(0.2, 0.7)).unwrap(), 1);
        session.update();

        assert!(filled_at(&session, Vec2::new(0.2, 0.7)));
        assert!(!filled_at(&session, Vec2::new(0.7, 0.2)), "the quad's other triangle stays");
        assert!(session.polygon_fill_uv(Vec2::new(1.5, 0.5)).is_err(), "nothing outside the UVs");
    }
}
