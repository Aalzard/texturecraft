//! The Path tool (stroked and filled paths, 3D and 2D) and the particle brush, driven through
//! engine commands the way agents paint.

use serde_json::{Value, json};
use texturecraft_engine::core::Channel;
use texturecraft_engine::geom::glam::{Vec2, Vec3, Vec4};
use texturecraft_engine::geom::{Mesh, primitives};
use texturecraft_engine::{Session, execute};

const RES: f32 = 256.0;

fn session(mesh: Mesh) -> Session {
    let mut s = Session::new_project("t", mesh, "builtin:test", None, 256).unwrap();
    s.viewport = Vec2::new(400.0, 400.0);
    s.camera.yaw = 0.0;
    s.camera.pitch = 0.0;
    s.update();
    execute(&mut s, "tool.material", &json!({"channel": "baseColor", "color": [1.0, 0.0, 0.0]})).unwrap();
    execute(&mut s, "tool.brush", &json!({"size": 6, "hardness": 90})).unwrap();
    s
}

fn red_uv(s: &Session, u: f32, v: f32) -> bool {
    let c = s.caches[0].composite.value(Channel::BaseColor, (u * RES) as u32, (v * RES) as u32);
    c[0] > 0.85 && c[1] < 0.15
}

/// Is the surface under a viewport point painted red?
fn red_at(s: &Session, x: f32, y: f32) -> bool {
    let hit = s.pick(Vec2::new(x, y), s.viewport).unwrap();
    red_uv(s, hit.uv.x, hit.uv.y)
}

#[test]
fn path_stroke_paints_along_the_curve() {
    let mut s = session(primitives::sphere(1.0, 48, 24));
    let r = execute(&mut s, "tool.path", &json!({"points": [[140, 210], [200, 180], [260, 210]]})).unwrap();
    assert_eq!(r["points"], 3);
    assert_eq!(s.tool.tool, texturecraft_engine::Tool::Path);
    assert!(!red_at(&s, 200.0, 180.0), "nothing painted before commit");
    let r = execute(&mut s, "tool.path", &json!({"commit": true})).unwrap();
    assert!(r["committed"]["stamps"].as_u64().unwrap() > 10, "{r}");
    assert_eq!(r["points"], 0, "commit clears the path");
    for (x, y) in [(140.0, 210.0), (200.0, 180.0), (260.0, 210.0), (170.0, 191.0)] {
        assert!(red_at(&s, x, y), "on the curve at {x},{y}");
    }
    assert!(!red_at(&s, 200.0, 240.0), "below the curve");
    assert!(!red_at(&s, 200.0, 130.0), "above the curve");
    // One undo step.
    execute(&mut s, "edit.undo", &Value::Null).unwrap();
    s.update();
    assert!(!red_at(&s, 200.0, 180.0));
    // Errors: no points, points off the mesh.
    assert!(execute(&mut s, "tool.path", &json!({"commit": true})).is_err());
    assert!(execute(&mut s, "tool.path", &json!({"points": [[2, 2]]})).is_err());
}

#[test]
fn uv_path_strokes_in_texture_space() {
    let mut s = session(primitives::sphere(1.0, 48, 24));
    execute(&mut s, "tool.path", &json!({"uvPoints": [[0.2, 0.5], [0.5, 0.45], [0.8, 0.5]], "commit": true})).unwrap();
    assert!(red_uv(&s, 0.5, 0.45) && red_uv(&s, 0.2, 0.5) && red_uv(&s, 0.8, 0.5));
    assert!(!red_uv(&s, 0.5, 0.6));
}

#[test]
fn closed_filled_path_fills_the_inside_only() {
    let mut s = session(primitives::sphere(1.0, 48, 24));
    let square = json!([[160, 160], [240, 160], [240, 240], [160, 240]]);
    execute(&mut s, "tool.path", &json!({"points": square, "closed": true, "fill": true, "commit": true})).unwrap();
    for (x, y) in [(200.0, 200.0), (170.0, 170.0), (230.0, 230.0)] {
        assert!(red_at(&s, x, y), "inside at {x},{y}");
    }
    for (x, y) in [(200.0, 125.0), (275.0, 200.0), (125.0, 200.0), (200.0, 275.0)] {
        assert!(!red_at(&s, x, y), "outside at {x},{y}");
    }
    // The far side of the sphere, hidden from the camera, stays clean.
    s.camera.yaw = std::f32::consts::PI;
    assert!(!red_at(&s, 200.0, 200.0), "hidden back side");

    // A 2D lasso fills the UV region.
    let mut s = session(primitives::sphere(1.0, 48, 24));
    execute(&mut s, "tool.path", &json!({"uvPoints": [[0.3, 0.3], [0.7, 0.3], [0.7, 0.7], [0.3, 0.7]], "closed": true, "fill": true, "commit": true})).unwrap();
    assert!(red_uv(&s, 0.5, 0.5) && red_uv(&s, 0.35, 0.65));
    assert!(!red_uv(&s, 0.2, 0.5) && !red_uv(&s, 0.5, 0.85));
    // Options persist between paths; a filled path needs three points.
    assert!(execute(&mut s, "tool.path", &json!({"uvPoints": [[0.1, 0.1], [0.2, 0.2]], "closed": false, "commit": true})).is_ok());
    assert!(execute(&mut s, "tool.path", &json!({"uvPoints": [[0.1, 0.1], [0.2, 0.2]], "closed": true, "fill": true, "commit": true})).is_err());
}

/// A 2×2 wall facing the camera (+Z): v grows downward (world −Y).
fn wall() -> Mesh {
    let mut m = primitives::plane(2.0, 8);
    for p in &mut m.positions {
        *p = Vec3::new(p.x, -p.z, 0.0);
    }
    m.normals.iter_mut().for_each(|n| *n = Vec3::Z);
    m.tangents.iter_mut().for_each(|t| *t = Vec4::new(1.0, 0.0, 0.0, 1.0));
    m
}

/// Painted texel rows (v) after one particle dab at the wall's center.
fn particle_rows() -> Vec<u32> {
    let mut s = session(wall());
    execute(&mut s, "tool.brush", &json!({"size": 10, "particles": {"enabled": true, "count": 6, "lifetime": 40, "gravity": 1.5}})).unwrap();
    let hit = s.pick(Vec2::new(200.0, 200.0), s.viewport).unwrap();
    assert!((hit.uv.y - 0.5).abs() < 0.02, "{hit:?}");
    execute(&mut s, "tool.stroke", &json!({"points": [[200, 200]]})).unwrap();
    let mut rows = Vec::new();
    for y in 0..RES as u32 {
        for x in 0..RES as u32 {
            let c = s.caches[0].composite.value(Channel::BaseColor, x, y);
            if c[0] > 0.85 && c[1] < 0.5 {
                rows.push(y);
            }
        }
    }
    rows
}

#[test]
fn particle_brush_paints_below_the_start_and_is_deterministic() {
    let rows = particle_rows();
    assert!(!rows.is_empty());
    let start = (0.5 * RES) as u32;
    let lowest = rows.iter().max().copied().unwrap();
    let highest = rows.iter().min().copied().unwrap();
    assert!(lowest > start + 40, "trails run down the wall: {lowest}");
    assert!(highest + 20 > start, "nothing well above the start: {highest}");
    assert_eq!(rows, particle_rows());
}

#[test]
fn radial_symmetry_paints_copies_around_the_axis() {
    let mut s = session(primitives::sphere(1.0, 48, 24));
    assert!(execute(&mut s, "tool.brush", &json!({"symmetryRadial": 2, "symmetryAxis": "w"})).is_err());
    let r = execute(&mut s, "tool.brush", &json!({"symmetryRadial": 2, "symmetryAxis": "y"})).unwrap();
    assert_eq!(r["radialSymmetry"], json!({"copies": 2, "axis": "y"}));
    execute(&mut s, "tool.stroke", &json!({"points": [[200, 200]]})).unwrap();
    s.update();
    assert!(red_at(&s, 200.0, 200.0));
    // Half a turn around Y: the copy faces the other way.
    s.camera.yaw = std::f32::consts::PI;
    assert!(red_at(&s, 200.0, 200.0), "the radial copy on the back");
}
