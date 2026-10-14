//! Bake commands: settings, high-definition meshes, cage, and project round trips.

use serde_json::{Value, json};
use texturecraft_engine::core::{BakeMatch, BakedMap};
use texturecraft_engine::geom::{Mesh, primitives};
use texturecraft_engine::{Session, execute};

fn plane_session() -> Session {
    let mesh = primitives::builtin("plane").unwrap();
    Session::new_project("t", mesh, "builtin:plane", None, 256).unwrap()
}

/// `mesh` as OBJ text, lifted by `dy`, keeping only vertices with x ≤ `max_x` (triangles that
/// lose a vertex are dropped), every vertex colored `rgb`.
fn obj(mesh: &Mesh, dy: f32, max_x: f32, rgb: [f32; 3]) -> String {
    let mut s = String::from("o Part_high\n");
    for p in &mesh.positions {
        s += &format!("v {} {} {} {} {} {}\n", p.x.min(max_x), p.y + dy, p.z, rgb[0], rgb[1], rgb[2]);
    }
    for t in mesh.indices.chunks_exact(3) {
        s += &format!("f {} {} {}\n", t[0] + 1, t[1] + 1, t[2] + 1);
    }
    s
}

fn temp_file(name: &str, text: &str) -> String {
    let dir = std::env::temp_dir().join(format!("texturecraft-bake-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path.display().to_string()
}

#[test]
fn high_poly_meshes_drive_the_bake_and_round_trip() {
    let mut s = plane_session();
    let low = s.mesh.as_ref().clone();
    let (lo, hi) = low.bounds();
    let mid = (lo.x + hi.x) * 0.5;
    // A red high poly over the left half of the plane, just above it.
    let high = temp_file("hp_high.obj", &obj(&low, 0.002, mid, [1.0, 0.0, 0.0]));
    let r = execute(&mut s, "bake.highPoly.add", &json!({ "path": high })).unwrap();
    assert_eq!(r["name"], "hp_high.obj");
    assert!(s.doc.project.texture_sets.iter().all(|t| !t.bake.use_low_as_high), "adding a high poly bakes from it");
    let list = execute(&mut s, "bake.highPoly.list", &Value::Null).unwrap();
    assert_eq!(list["highPoly"][0]["parts"][0], "Part_high");
    assert!(execute(&mut s, "project.inspect", &Value::Null).unwrap()["bakeMeshes"]["highPoly"].as_array().is_some_and(|a| a.len() == 1));

    execute(&mut s, "bake.run", &json!({ "resolution": 128, "maps": ["opacity", "color_map"], "dilation": 0 })).unwrap();
    let baked = &s.doc.baked[&0];
    let opacity = &baked.maps[&BakedMap::Opacity];
    let covered = opacity.iter().filter(|v| **v > 0.99).count() as f32 / opacity.len() as f32;
    assert!((covered - 0.5).abs() < 0.08, "the high poly covers half the plane: {covered}");
    let color = &baked.maps[&BakedMap::ColorMap];
    assert!(opacity.iter().zip(color.chunks(3)).filter(|(o, _)| **o > 0.99).all(|(_, c)| c == [1.0, 0.0, 0.0]), "red where covered");

    // A cage must have the low poly's triangles.
    let tri = temp_file("tri.obj", "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n");
    assert!(execute(&mut s, "bake.cage.set", &json!({ "path": tri })).is_err());
    let cage = temp_file("cage.obj", &obj(&low, 0.05, f32::MAX, [1.0; 3]));
    execute(&mut s, "bake.cage.set", &json!({ "path": cage })).unwrap();
    execute(&mut s, "bake.settings", &json!({ "matching": "byMeshName", "maxFrontal": 0.05 })).unwrap();

    // Save and reopen: meshes, cage and settings come back.
    let back = Session::from_project_bytes(&s.to_project_bytes().unwrap()).unwrap();
    let names = |s: &Session| s.bake_meshes.high.iter().map(|h| (h.name.clone(), h.mesh.triangle_count())).collect::<Vec<_>>();
    assert_eq!(names(&back), names(&s));
    assert_eq!(back.bake_meshes.cage.as_ref().map(|c| c.name.as_str()), Some("cage.obj"));
    assert_eq!(back.doc.project.texture_sets[0].bake, s.doc.project.texture_sets[0].bake);
    assert_eq!(back.doc.project.texture_sets[0].bake.matching, BakeMatch::ByMeshName);

    execute(&mut s, "bake.highPoly.remove", &json!({ "name": "hp_high.obj" })).unwrap();
    assert!(s.bake_meshes.high.is_empty());
    execute(&mut s, "bake.cage.clear", &Value::Null).unwrap();
    assert!(s.bake_meshes.cage.is_none());
    // Without high polys the bake falls back to the low poly.
    execute(&mut s, "bake.run", &json!({ "resolution": 128, "maps": ["opacity"] })).unwrap();
    assert!(s.doc.baked[&0].maps[&BakedMap::Opacity].iter().all(|v| *v == 1.0));
    let _ = std::fs::remove_dir_all(std::path::Path::new(&high).parent().unwrap());
}

#[test]
fn malformed_high_poly_input_errors_cleanly() {
    let mut s = plane_session();
    assert!(s.add_high_poly_bytes("bad.obj", b"this is not a mesh".to_vec()).is_err());
    assert!(s.add_high_poly_bytes("bad.glb", b"glTF\x02\0\0\0\xff\xff\xff\xff".to_vec()).is_err());
    assert!(s.add_high_poly_bytes("bad.xyz", vec![1, 2, 3]).is_err());
    assert!(s.set_cage_bytes("bad.stl", vec![0; 10]).is_err());
    assert!(execute(&mut s, "bake.highPoly.add", &json!({ "path": "/definitely/missing.obj" })).is_err());
    assert!(execute(&mut s, "bake.highPoly.remove", &json!({ "index": 3 })).is_err());
    assert!(execute(&mut s, "bake.highPoly.remove", &Value::Null).is_err());
    assert!(s.bake_meshes.high.is_empty() && s.bake_meshes.cage.is_none());
    assert!(s.doc.project.texture_sets.iter().all(|t| t.bake.use_low_as_high), "failed adds change nothing");
}

#[test]
fn bake_settings_apply_atomically() {
    let mut s = Session::new_project("t", primitives::material_ball(), "builtin:material_ball", None, 256).unwrap();
    let r =
        execute(&mut s, "bake.settings", &json!({ "ts": "all", "antialiasing": 4, "direction": [1, 0, 0], "maps": ["bent_normals", "World space direction"] }))
            .unwrap();
    assert_eq!(r.as_array().map(Vec::len), Some(2));
    for t in &s.doc.project.texture_sets {
        assert_eq!((t.bake.antialiasing, t.bake.direction), (4, [1.0, 0.0, 0.0]));
        assert_eq!(t.bake.maps.iter().copied().collect::<Vec<_>>(), [BakedMap::BentNormal, BakedMap::WorldDirection]);
    }
    let before = s.doc.project.texture_sets[0].bake.clone();
    for bad in [
        json!({ "rays": 4, "antialiasing": 3 }),
        json!({ "maps": ["nope"] }),
        json!({ "direction": [0, 0, 0] }),
        json!({ "matching": "?" }),
        json!({ "ts": [0, 9] }),
    ] {
        assert!(execute(&mut s, "bake.settings", &bad).is_err(), "{bad}");
        assert_eq!(s.doc.project.texture_sets[0].bake, before, "{bad} changed nothing");
    }
    // "selected" = the sets checked for baking.
    execute(&mut s, "bake.settings", &json!({ "ts": 1, "enabled": false })).unwrap();
    let r = execute(&mut s, "bake.settings", &json!({ "ts": "selected", "rays": 8 })).unwrap();
    assert_eq!(r[0]["ts"], 0);
    assert_eq!(r.as_array().map(Vec::len), Some(1));
    // The returned parameters set the same settings again (what the UI sends back).
    let mut copy = r[0]["settings"].clone();
    copy["ts"] = 1.into();
    execute(&mut s, "bake.settings", &copy).unwrap();
    let (a, b) = (&s.doc.project.texture_sets[0].bake, &s.doc.project.texture_sets[1].bake);
    assert_eq!(a, b);
}
