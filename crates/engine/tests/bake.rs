//! Bake commands: settings, high-definition meshes, cage, and project round trips.

use serde_json::{Value, json};
use texturecraft_engine::core::{BakeMatch, BakedMap, DirectionMode, PositionMode, RayDistribution, RaySettings, SelfOcclusion};
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

#[test]
fn per_map_ray_settings_round_trip() {
    let mut s = plane_session();
    let p = json!({
        "ao": { "rays": 48, "maxDistance": 0.5, "spread": 90, "distribution": "uniform", "ignoreBackface": true, "selfOcclusion": "sameMeshName" },
        "thickness": { "rays": 12, "spread": 45 },
        "bentNormal": { "rays": 6, "maxDistance": 0.1 },
        "curvatureRadius": 3,
        "positionMode": "boundingSphere",
        "directionMode": "positive",
    });
    let r = execute(&mut s, "bake.settings", &p).unwrap();
    let b = s.doc.project.texture_sets[0].bake.clone();
    assert_eq!(b.ao, RaySettings { rays: 48, max_distance: 0.5, spread: 90.0, distribution: RayDistribution::Uniform });
    assert!(b.ao_ignore_backface && b.ao_self_occlusion == SelfOcclusion::SameMeshName);
    assert_eq!((b.thickness.rays, b.thickness.spread, b.thickness.max_distance), (12, 45.0, 0.1));
    assert_eq!((b.bent_normal.rays, b.bent_normal.max_distance, b.bent_normal.spread), (6, 0.1, 180.0));
    assert_eq!((b.curvature_radius, b.position_mode, b.direction_mode), (3, PositionMode::BoundingSphere, DirectionMode::Positive));
    assert_eq!(r[0]["settings"]["ao"]["selfOcclusion"], "sameMeshName");
    assert_eq!(r[0]["settings"]["thickness"]["distribution"], "cosine");
    // The project file keeps them.
    let back = Session::from_project_bytes(&s.to_project_bytes().unwrap()).unwrap();
    assert_eq!(back.doc.project.texture_sets[0].bake, b);
    // `rays` and `maxDistance` set every ray map; unknown names change nothing.
    execute(&mut s, "bake.settings", &json!({ "rays": 16, "maxDistance": 0.2 })).unwrap();
    let b = s.doc.project.texture_sets[0].bake.clone();
    assert!([b.ao, b.thickness, b.bent_normal].iter().all(|r| r.rays == 16 && r.max_distance == 0.2));
    for bad in [
        json!({ "rays": 8, "ao": { "distribution": "gaussian" } }),
        json!({ "ao": { "selfOcclusion": "never" } }),
        json!({ "thickness": { "distribution": 3, "rays": 2 }, "positionMode": "uv" }),
        json!({ "directionMode": "absolute" }),
    ] {
        assert!(execute(&mut s, "bake.settings", &bad).is_err(), "{bad}");
        assert_eq!(s.doc.project.texture_sets[0].bake, b, "{bad} changed nothing");
    }
    // Forcing the CPU bakes there.
    let r = execute(&mut s, "bake.run", &json!({ "resolution": 128, "maps": ["ambient_occlusion"], "cpu": true })).unwrap();
    assert_eq!(r["tracer"], json!(["CPU"]));
}

/// A 4×4 PNG in quadrants: red, green / blue, mid grey (top row = v 0).
fn quadrants_png() -> Vec<u8> {
    let quadrant = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [128, 128, 128, 255]];
    let img = image::RgbaImage::from_fn(4, 4, |x, y| image::Rgba(quadrant[(y / 2 * 2 + x / 2) as usize]));
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    png
}

/// The baked 128 px color map of set 0 shows [`quadrants_png`] on the plane.
fn assert_quadrants(s: &Session) {
    let color = &s.doc.baked[&0].maps[&BakedMap::ColorMap];
    let at = |x: usize, y: usize| &color[(y * 128 + x) * 3..(y * 128 + x) * 3 + 3];
    let grey = 128.0 / 255.0;
    for ((x, y), want) in [((32, 32), [1.0, 0.0, 0.0]), ((96, 32), [0.0, 1.0, 0.0]), ((32, 96), [0.0, 0.0, 1.0]), ((96, 96), [grey; 3])] {
        assert!(at(x, y).iter().zip(want).all(|(a, b)| (a - b).abs() < 2e-3), "texel {x},{y}: {:?} vs {want:?}", at(x, y));
    }
}

/// A GLB of `mesh` lifted by `dy`: one part `Part_high` whose material's base color is `png`.
fn textured_glb(mesh: &Mesh, dy: f32, png: &[u8]) -> Vec<u8> {
    let mut bin = Vec::new();
    let mut view = |bytes: &[u8]| {
        let offset = bin.len();
        bin.extend_from_slice(bytes);
        bin.resize(bin.len().next_multiple_of(4), 0);
        json!({ "buffer": 0, "byteOffset": offset, "byteLength": bytes.len() })
    };
    let floats = |v: Vec<f32>| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let views = [
        view(&floats(mesh.positions.iter().flat_map(|p| [p.x, p.y + dy, p.z]).collect())),
        view(&floats(mesh.uvs.iter().flat_map(|uv| [uv.x, uv.y]).collect())),
        view(&mesh.indices.iter().flat_map(|i| i.to_le_bytes()).collect::<Vec<u8>>()),
        view(png),
    ];
    let (lo, hi) = mesh.bounds();
    let doc = json!({
        "asset": { "version": "2.0" },
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [{ "name": "Part_high", "mesh": 0 }],
        "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0, "TEXCOORD_0": 1 }, "indices": 2, "material": 0 }] }],
        "materials": [{ "name": "Painted", "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } } }],
        "textures": [{ "source": 0 }],
        "images": [{ "bufferView": 3, "mimeType": "image/png" }],
        "buffers": [{ "byteLength": bin.len() }],
        "bufferViews": views,
        "accessors": [
            { "bufferView": 0, "componentType": 5126, "count": mesh.positions.len(), "type": "VEC3", "min": [lo.x, lo.y + dy, lo.z], "max": [hi.x, hi.y + dy, hi.z] },
            { "bufferView": 1, "componentType": 5126, "count": mesh.positions.len(), "type": "VEC2" },
            { "bufferView": 2, "componentType": 5125, "count": mesh.indices.len(), "type": "SCALAR" }
        ]
    });
    let mut text = serde_json::to_vec(&doc).unwrap();
    text.resize(text.len().next_multiple_of(4), b' ');
    let mut glb = Vec::new();
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&((12 + 8 + text.len() + 8 + bin.len()) as u32).to_le_bytes());
    for (chunk, kind) in [(&text, b"JSON"), (&bin, b"BIN\0")] {
        glb.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        glb.extend_from_slice(kind);
        glb.extend_from_slice(chunk);
    }
    glb
}

#[test]
fn color_map_samples_a_textured_gltf_high_poly() {
    let mut s = plane_session();
    let low = s.mesh.as_ref().clone();
    let png = quadrants_png();
    s.add_high_poly_bytes("painted.glb", textured_glb(&low, 0.002, &png)).unwrap();
    assert!(s.log.iter().any(|l| l.contains("1 base color textures")), "{:?}", s.log);
    execute(&mut s, "bake.run", &json!({ "resolution": 128, "maps": ["color_map", "opacity"], "dilation": 0 })).unwrap();
    assert_quadrants(&s);
    assert!(s.doc.baked[&0].maps[&BakedMap::Opacity].iter().all(|o| *o > 0.99), "the high poly covers the plane");
    // The textures come back with the project.
    let back = Session::from_project_bytes(&s.to_project_bytes().unwrap()).unwrap();
    assert_eq!(back.bake_meshes.high[0].base_color, s.bake_meshes.high[0].base_color);
}

/// The color map reads a USDZ high poly's UsdPreviewSurface texture like a glTF one.
#[test]
fn color_map_samples_a_textured_usdz_high_poly() {
    use texturecraft_engine::formats::{ExportedFile, UsdFormat, usd_bytes};
    let mut s = plane_session();
    let mut high = s.mesh.as_ref().clone();
    for p in &mut high.positions {
        p.y += 0.002;
    }
    let png = quadrants_png();
    let texture = ExportedFile { name: "BaseColor.png".into(), bytes: png, width: 4, height: 4, warnings: vec![] };
    let usdz = usd_bytes(&high, "painted", &[vec![texture]], UsdFormat::Usdz).unwrap().bytes;
    s.add_high_poly_bytes("painted.usdz", usdz).unwrap();
    execute(&mut s, "bake.run", &json!({ "resolution": 128, "maps": ["color_map"], "dilation": 0 })).unwrap();
    assert_quadrants(&s);
}

/// High polys whose textures are files next to them (a `.gltf` with its `.bin` and image, a loose
/// usda) are folded into one file on add, bake their color map, and come back with the project.
#[test]
fn color_map_samples_textures_next_to_a_high_poly() {
    use texturecraft_engine::formats::{ExportedFile, UsdFormat, usd_bytes};
    let dir = std::env::temp_dir().join(format!("texturecraft-bake-external-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("tex")).unwrap();
    let low = plane_session().mesh.as_ref().clone();
    // The textured GLB taken apart: JSON, its .bin and the image as files.
    let glb = textured_glb(&low, 0.002, &quadrants_png());
    let jlen = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    let mut doc: Value = serde_json::from_slice(&glb[20..20 + jlen]).unwrap();
    doc["buffers"][0]["uri"] = "painted.bin".into();
    doc["images"] = json!([{ "uri": "tex/painted%20color.png" }]);
    std::fs::write(dir.join("painted.bin"), &glb[20 + jlen + 8..]).unwrap();
    std::fs::write(dir.join("tex/painted color.png"), quadrants_png()).unwrap();
    std::fs::write(dir.join("painted.gltf"), serde_json::to_vec(&doc).unwrap()).unwrap();
    // The same high poly as a usda with its texture beside it.
    let mut high = low.clone();
    for p in &mut high.positions {
        p.y += 0.002;
    }
    let texture = ExportedFile { name: "BaseColor.png".into(), bytes: quadrants_png(), width: 4, height: 4, warnings: vec![] };
    let usda = usd_bytes(&high, "painted", &[vec![texture]], UsdFormat::Usda).unwrap();
    for (name, bytes) in &usda.textures {
        std::fs::write(dir.join(name), bytes).unwrap();
    }
    std::fs::write(dir.join("painted.usda"), &usda.bytes).unwrap();
    for (file, stored) in [("painted.gltf", "painted.gltf"), ("painted.usda", "painted.usdz")] {
        let mut s = plane_session();
        let r = execute(&mut s, "bake.highPoly.add", &json!({ "path": dir.join(file).display().to_string() })).unwrap();
        assert_eq!(r["name"], stored);
        assert!(s.log.iter().any(|l| l.contains("1 base color textures")), "{file}: {:?}", s.log);
        execute(&mut s, "bake.run", &json!({ "resolution": 128, "maps": ["color_map"], "dilation": 0 })).unwrap();
        assert_quadrants(&s);
        // The project holds the folded file: the textures come back without the folder.
        let back = Session::from_project_bytes(&s.to_project_bytes().unwrap()).unwrap();
        assert_eq!(back.bake_meshes.high[0].base_color, s.bake_meshes.high[0].base_color, "{file}");
    }
    // A missing image is named in the log; the high poly still loads, untextured.
    std::fs::remove_file(dir.join("tex/painted color.png")).unwrap();
    let mut s = plane_session();
    execute(&mut s, "bake.highPoly.add", &json!({ "path": dir.join("painted.gltf").display().to_string() })).unwrap();
    assert!(s.log.iter().any(|l| l.contains("tex/painted color.png was not found")), "{:?}", s.log);
    std::fs::remove_dir_all(&dir).unwrap();
}
