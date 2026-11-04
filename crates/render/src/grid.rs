//! The viewport's ground grid: settings and the camera-dependent numbers the grid pass
//! (`shaders/grid.wgsl`) needs. The pass draws a large quad on the ground plane, so the lines are
//! procedural (no geometry per line), depth-tested against the mesh and anti-aliased with
//! screen-space derivatives. It is a viewport overlay only: it exists nowhere but in
//! [`Shade::grid`](crate::Shade) of the 3D view, and every other path (CLI renders, the path
//! tracer, bakes, texture export) never sets it.

use texturecraft_geom::OrbitCamera;

/// How the 3D view draws the ground grid (`None` in [`Shade`](crate::Shade): no grid).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridShade {
    /// Overall strength, 0..1 (0 draws nothing).
    pub opacity: f32,
    /// Put the grid at the model's lowest point instead of at height 0.
    pub on_model_floor: bool,
}

impl Default for GridShade {
    fn default() -> Self {
        GridShade { opacity: 0.6, on_model_floor: false }
    }
}

impl GridShade {
    /// Whether this setting draws anything.
    pub fn visible(&self) -> bool {
        self.opacity.is_finite() && self.opacity > 0.0
    }
}

/// The grid numbers for one camera, in the order the shader reads them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GridParams {
    /// Spacing of the finest lines (world units); the major lines are 10× and 100× that.
    pub spacing: f32,
    /// 0..1: how far the zoom has moved toward the next decade (the finest lines fade out as
    /// the coarser ones take over, so the grid never pops).
    pub blend: f32,
    /// Distance from the eye where the grid starts to fade out, and where it is gone.
    pub fade: [f32; 2],
    /// Half the side of the quad (it is centred under the eye and reaches past the fade).
    pub half: f32,
    /// View depths where the grid is gone and where it is fully there: the camera's near plane
    /// clips the ground (it sits close to the model when zoomed out), and the grid fades in
    /// past it instead of ending in a straight edge. Orthographic views have no such edge.
    pub near: [f32; 2],
}

/// Opaque spacing of the finest lines and the blend toward the next decade for a camera
/// `distance` from its target. One decade of zoom changes the spacing by exactly 10×, so the
/// grid always shows roughly 10 to 100 cells across the model, whatever its size.
pub(crate) fn decade(distance: f32) -> (f32, f32) {
    let d = if distance.is_finite() { distance.clamp(1e-4, 1e7) } else { 1.0 };
    let t = d.log10() - 1.0;
    let floor = t.floor();
    (10f32.powf(floor), (t - floor).clamp(0.0, 1.0))
}

impl GridParams {
    pub(crate) fn new(cam: &OrbitCamera) -> GridParams {
        let (spacing, blend) = decade(cam.distance);
        let finite = |v: f32, lo: f32, hi: f32| if v.is_finite() { v.clamp(lo, hi) } else { 1.0 };
        let (distance, radius) = (finite(cam.distance, 1e-4, 1e7), finite(cam.scene_radius, 1e-3, 1e7));
        // The camera's far plane (see `OrbitCamera::proj`); the grid is gone before it so the
        // quad's far edge can never show as a line.
        let far = distance + radius * 50.0;
        let end = (far * 0.9).min(distance * 10.0);
        // The camera's near plane (see `OrbitCamera::proj`).
        let near_plane = (distance - radius * 4.0).max(radius * 0.01).max(1e-4);
        let near = if cam.ortho { [-2.0, -1.0] } else { [near_plane, near_plane + distance * 0.3] };
        GridParams { spacing, blend, fade: [end * 0.25, end], half: end, near }
    }
}

/// The height of the grid plane: `nominal` (0, or the model's lowest point) a hair lower, so
/// a model standing on the grid does not z-fight with it.
pub(crate) fn plane_height(nominal: f32, radius: f32) -> f32 {
    nominal - radius.max(1e-3) * 1e-3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::{MESH_WGSL, POST_WGSL, SHADOW_WGSL, UTIL_WGSL};

    /// Parse and validate one shader module, the way wgpu does before it builds a pipeline.
    fn validate(name: &str, source: &str) -> naga::Module {
        let module = naga::front::wgsl::parse_str(source).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
        module
    }

    #[test]
    fn the_viewport_shaders_validate() {
        validate("util", UTIL_WGSL);
        validate("post", POST_WGSL);
        validate("shadow", SHADOW_WGSL);
        validate("mesh", MESH_WGSL);
    }

    #[test]
    fn the_grid_entry_points_exist() {
        let module = validate("mesh", MESH_WGSL);
        for (name, stage) in [("vs_grid", naga::ShaderStage::Vertex), ("fs_grid", naga::ShaderStage::Fragment)] {
            assert!(module.entry_points.iter().any(|e| e.name == name && e.stage == stage), "missing {name}");
        }
    }

    #[test]
    fn spacing_follows_the_zoom_by_decades() {
        for (distance, spacing) in [(10.0, 1.0), (99.0, 1.0), (100.0, 10.0), (3.0, 0.1), (0.5, 0.01), (2500.0, 100.0)] {
            let (s, _) = decade(distance);
            assert!((s / spacing - 1.0).abs() < 1e-3, "distance {distance}: spacing {s}, want {spacing}");
        }
        // The blend runs 0 → 1 across a decade and wraps exactly where the spacing changes.
        let (s0, b0) = decade(10.0);
        let (s1, b1) = decade(99.99);
        let (s2, b2) = decade(100.0);
        assert!(b0 < 0.01 && b1 > 0.99 && b2 < 0.01, "{b0} {b1} {b2}");
        assert!((s1 - s0).abs() < 1e-6 && (s2 / s1 - 10.0).abs() < 1e-3);
    }

    #[test]
    fn hostile_distances_stay_finite() {
        for d in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -5.0, 1e30] {
            let (s, b) = decade(d);
            assert!(s.is_finite() && s > 0.0 && (0.0..=1.0).contains(&b), "{d}: {s} {b}");
            let p = GridParams::new(&OrbitCamera { distance: d, scene_radius: d, ..OrbitCamera::default() });
            assert!(p.half.is_finite() && p.half > 0.0 && p.fade.iter().all(|f| f.is_finite()), "{d}: {p:?}");
        }
    }

    #[test]
    fn the_fade_ends_before_the_far_plane() {
        for (distance, radius) in [(5.0, 1.0), (2.5, 1.0), (40.0, 0.5), (0.2, 100.0)] {
            let cam = OrbitCamera { distance, scene_radius: radius, ..OrbitCamera::default() };
            let p = GridParams::new(&cam);
            let far = distance + radius * 50.0;
            assert!(p.fade[1] <= far * 0.9 + 1e-3, "fade ends at {} but far is {far}", p.fade[1]);
            assert!(p.fade[0] < p.fade[1] && p.half >= p.fade[1]);
            // Fully in before the fade-out starts, and past the camera's near plane.
            assert!(p.near[0] < p.near[1] && p.near[1] < p.fade[1], "{p:?}");
        }
    }

    #[test]
    fn the_grid_fades_in_past_the_near_plane() {
        // Zoomed far out the near plane is close to the model: the ground in front of it is
        // clipped, so the grid must be gone at the plane and only then ramp up.
        let cam = OrbitCamera { distance: 72.0, scene_radius: 1.0, ..OrbitCamera::default() };
        let near_plane = cam.proj(1.0).inverse().project_point3(texturecraft_geom::glam::Vec3::new(0.0, 0.0, 0.0)).length();
        let p = GridParams::new(&cam);
        assert!((p.near[0] - near_plane).abs() < 0.5, "grid near {} vs projection near {near_plane}", p.near[0]);
        assert!(GridParams::new(&OrbitCamera { ortho: true, ..cam }).near[1] < 0.0);
    }

    #[test]
    fn the_plane_sits_just_under_the_model() {
        assert!(plane_height(0.0, 2.0) < 0.0 && plane_height(0.0, 2.0) > -0.01);
        assert!(plane_height(-3.0, 1.0) < -3.0);
    }

    #[test]
    fn nothing_but_the_viewport_sets_the_grid() {
        use crate::renderer::{Frame, Shade, ViewKind};
        use texturecraft_core::{DisplaySettings, Project};
        // Every constructor the CLI, exports and renders go through leaves the grid off.
        assert_eq!(Shade::default().grid, None);
        assert_eq!(Shade::from_display(&DisplaySettings::default()).grid, None);
        assert_eq!(Shade::from_project(&Project::new("t", "builtin:sphere")).grid, None);
        let frame =
            |kind, grid| Frame { view_id: 1, size: [8, 8], kind, camera: OrbitCamera::default(), shade: Shade { grid, ..Shade::default() }, visible: vec![] };
        assert!(!frame(ViewKind::Scene, None).grid_on());
        assert!(frame(ViewKind::Scene, Some(GridShade::default())).grid_on());
        let uv = ViewKind::Uv { ts: 0, zoom: 1.0, pan: [0.0; 2] };
        assert!(!frame(uv, Some(GridShade::default())).grid_on(), "the 2D view never has a grid");
    }

    #[test]
    fn zero_opacity_is_invisible() {
        assert!(GridShade::default().visible());
        assert!(!GridShade { opacity: 0.0, ..GridShade::default() }.visible());
        assert!(!GridShade { opacity: f32::NAN, ..GridShade::default() }.visible());
    }
}
