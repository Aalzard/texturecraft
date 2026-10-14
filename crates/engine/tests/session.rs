use serde_json::{Value, json};
use texturecraft_engine::core::Channel;
use texturecraft_engine::geom::primitives;
use texturecraft_engine::{Session, command_specs, execute};

fn session() -> Session {
    let mut s = Session::new_project("t", primitives::sphere(1.0, 48, 24), "builtin:sphere", None, 256).unwrap();
    s.viewport = texturecraft_engine::geom::glam::Vec2::new(400.0, 400.0);
    s.camera.yaw = 0.0;
    s.camera.pitch = 0.0;
    s.update();
    s
}

#[test]
fn scripted_stroke_paints_and_undo_restores() {
    let mut s = session();
    execute(&mut s, "tool.material", &json!({"channel": "baseColor", "color": [1.0, 0.0, 0.0]})).unwrap();
    execute(&mut s, "tool.brush", &json!({"size": 20, "hardness": 90})).unwrap();
    let r = execute(&mut s, "tool.stroke", &json!({"points": [[180, 200], [220, 200]]})).unwrap();
    assert!(r["stamps"].as_u64().unwrap() > 1);
    // The texel under the screen center is now red.
    let hit = s.pick(texturecraft_engine::geom::glam::Vec2::new(200.0, 200.0), s.viewport).unwrap();
    let res = 256.0;
    let (x, y) = ((hit.uv.x * res) as u32, (hit.uv.y * res) as u32);
    let v = s.caches[0].composite.value(Channel::BaseColor, x, y);
    assert!(v[0] > 0.9 && v[1] < 0.1, "{v:?}");
    execute(&mut s, "edit.undo", &Value::Null).unwrap();
    s.update();
    let v = s.caches[0].composite.value(Channel::BaseColor, x, y);
    assert!((v[0] - 0.5).abs() < 0.01, "{v:?}");
    execute(&mut s, "edit.redo", &Value::Null).unwrap();
    s.update();
    assert!(s.caches[0].composite.value(Channel::BaseColor, x, y)[0] > 0.9);
}

#[test]
fn layers_masks_materials_and_bake() {
    let mut s = session();
    execute(&mut s, "layer.add.fill", &json!({"color": "#336699", "roughness": 0.2})).unwrap();
    execute(&mut s, "mask.add.black", &Value::Null).unwrap();
    execute(&mut s, "effect.add.generator", &json!({"generator": "position", "level": 0.5})).unwrap();
    execute(&mut s, "bake.run", &json!({"resolution": 128, "rays": 8})).unwrap();
    execute(&mut s, "material.apply", &json!({"material": "steel_painted_worn"})).unwrap();
    execute(&mut s, "layer.group", &Value::Null).unwrap();
    execute(&mut s, "layer.duplicate", &Value::Null).unwrap();
    let inspect = execute(&mut s, "project.inspect", &Value::Null).unwrap();
    assert!(inspect["layers"].as_array().unwrap().len() >= 3, "{inspect}");
    s.doc.project.validate().unwrap();
    s.update();
    for _ in 0..7 {
        execute(&mut s, "edit.undo", &Value::Null).unwrap();
    }
    assert_eq!(s.doc.project.walk(0).len(), 1, "back to the initial paint layer");
}

#[test]
fn polygon_fill_and_color_picker() {
    let mut s = session();
    execute(&mut s, "tool.material", &json!({"channel": "roughness", "value": 0.9})).unwrap();
    execute(&mut s, "tool.polygonFill", &Value::Null).unwrap();
    execute(&mut s, "tool.fillMode", &json!({"mode": "mesh"})).unwrap();
    execute(&mut s, "tool.click", &json!({"x": 200, "y": 200})).unwrap();
    execute(&mut s, "tool.colorPicker", &Value::Null).unwrap();
    let r = execute(&mut s, "tool.click", &json!({"x": 200, "y": 200})).unwrap();
    assert!((r["roughness"][0].as_f64().unwrap() - 0.9).abs() < 0.01, "{r}");
}

/// Every command must return Ok or Err — never panic — for hostile params.
#[test]
fn panic_hunt() {
    let hostile = [
        Value::Null,
        json!({}),
        json!({"layer": u64::MAX, "ts": 99, "channel": "nope", "mode": "???", "opacity": f64::MAX}),
        json!({"layer": 1, "index": -5, "delta": i64::MIN, "above": "x", "points": [[f64::MAX, -1e300]], "size": -1e30}),
        json!({"resolution": 0, "maps": ["x"], "rays": u64::MAX, "antialiasing": 3, "color": [1], "value": "nan"}),
        json!({"effect": 0, "x": 1e30, "y": -1e30, "viewport": [0, 0], "mesh": "../../etc", "material": ""}),
    ];
    for spec in command_specs() {
        if spec.id == "project.sample" {
            continue; // slow (bakes); covered elsewhere
        }
        for p in &hostile {
            let mut s = session();
            let _ = execute(&mut s, spec.id, p);
            s.update();
        }
    }
}

#[test]
fn copy_paste_flatten_and_resize_keep_the_look() {
    let mut s = Session::new_project("t", primitives::material_ball(), "builtin:material_ball", None, 256).unwrap();
    s.viewport = texturecraft_engine::geom::glam::Vec2::new(400.0, 400.0);
    execute(&mut s, "layer.add.fill", &json!({"color": [0.9, 0.1, 0.1], "roughness": 0.3})).unwrap();
    execute(&mut s, "mask.add.black", &Value::Null).unwrap();
    execute(&mut s, "effect.add.fill", &json!({"procedural": "checker", "scale": 4})).unwrap();
    s.update();
    let before: Vec<[f32; 3]> = (0..256).step_by(9).map(|x| s.caches[0].composite.value(Channel::BaseColor, x, 100)).collect();
    // Flatten keeps the composite identical (within 8-bit-ish tolerance) and makes a paint layer.
    let id = s.selected_layer().unwrap();
    execute(&mut s, "layer.flatten", &json!({"layer": id.0})).unwrap();
    s.update();
    let after: Vec<[f32; 3]> = (0..256).step_by(9).map(|x| s.caches[0].composite.value(Channel::BaseColor, x, 100)).collect();
    for (a, b) in before.iter().zip(&after) {
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() < 2e-3, "{a:?} {b:?}");
        }
    }
    assert!(matches!(s.doc.project.layer(id).unwrap().kind, texturecraft_engine::core::LayerKind::Paint));
    // Copy into the other texture set (different resolution).
    execute(&mut s, "textureSet.resolution", &json!({"ts": 1, "resolution": 128})).unwrap();
    execute(&mut s, "layer.copy", &Value::Null).unwrap();
    execute(&mut s, "textureSet.select", &json!({"ts": 1})).unwrap();
    let r = execute(&mut s, "layer.paste", &Value::Null).unwrap();
    let pasted = texturecraft_engine::core::LayerId(r["layer"].as_u64().unwrap());
    assert_eq!(s.doc.pixels.layer(pasted, Channel::BaseColor).unwrap().width(), 128);
    // Resizing resamples instead of clearing.
    execute(&mut s, "textureSet.resolution", &json!({"ts": 0, "resolution": 512})).unwrap();
    assert_eq!(s.doc.pixels.layer(id, Channel::BaseColor).unwrap().width(), 512);
    s.update();
    let big = s.caches[0].composite.value(Channel::BaseColor, 2 * 9 * 5, 200);
    let small = before[5];
    assert!((big[0] - small[0]).abs() < 0.2, "{big:?} {small:?}");
    execute(&mut s, "layer.instantiateAll", &json!({"layer": id.0})).unwrap();
    s.doc.project.validate().unwrap();
}

#[test]
fn image_resources_fill_layers_and_round_trip() {
    let mut s = session();
    let mut png = Vec::new();
    let img = image::RgbaImage::from_fn(8, 8, |_, _| image::Rgba([255, 0, 0, 255]));
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let name = s.import_resource_bytes("C:/x/red bricks.png", png).unwrap();
    assert_eq!(name, "red_bricks.png");
    let r = execute(&mut s, "layer.add.image", &json!({"name": name})).unwrap();
    assert!(r["layer"].as_u64().is_some());
    s.update();
    let v = s.caches[0].composite.value(Channel::BaseColor, 128, 128);
    assert!(v[0] > 0.95 && v[1] < 0.05, "{v:?}");
    let bytes = s.to_project_bytes().unwrap();
    let back = Session::from_project_bytes(&bytes).unwrap();
    assert_eq!(back.doc.resources.len(), 1);
    assert_eq!(back.images.len(), 1);
    execute(&mut s, "edit.undo", &Value::Null).unwrap();
    execute(&mut s, "edit.undo", &Value::Null).unwrap();
    assert!(s.images.is_empty(), "undoing the import forgets the decoded image");
    assert!(s.import_resource_bytes("bad.png", vec![1, 2, 3]).is_err());
}

#[test]
fn projection_tool_paints_the_stencil_colors() {
    let mut s = session();
    let mut png = Vec::new();
    let img = image::RgbaImage::from_fn(16, 16, |x, _| if x < 8 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 255, 0, 255]) });
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let name = s.import_resource_bytes("split.png", png).unwrap();
    execute(&mut s, "tool.projection", &Value::Null).unwrap();
    // Without a stencil the tool explains what's missing.
    assert!(execute(&mut s, "tool.stroke", &json!({"points": [[150, 200]]})).is_err());
    execute(&mut s, "tool.stencil", &json!({"image": name, "scale": 1.0, "tile": true})).unwrap();
    execute(&mut s, "tool.brush", &json!({"size": 6, "hardness": 90})).unwrap();
    for x in [150.0f32, 250.0] {
        execute(&mut s, "tool.stroke", &json!({"points": [[x, 200.0]]})).unwrap();
    }
    let vp = s.viewport;
    let at = |s: &Session, x: f32| {
        let hit = s.pick(texturecraft_engine::geom::glam::Vec2::new(x, 200.0), vp).unwrap();
        s.caches[0].composite.value(Channel::BaseColor, (hit.uv.x * 256.0) as u32, (hit.uv.y * 256.0) as u32)
    };
    let left = at(&s, 150.0);
    let right = at(&s, 250.0);
    assert!(left[0] > 0.8 && left[1] < 0.2, "left {left:?}");
    assert!(right[1] > 0.8 && right[0] < 0.2, "right {right:?}");
}

#[test]
fn smart_materials_save_and_load_with_pixels() {
    let mut s = Session::new_project("t", primitives::material_ball(), "builtin:material_ball", None, 256).unwrap();
    s.viewport = texturecraft_engine::geom::glam::Vec2::new(400.0, 400.0);
    let group = texturecraft_engine::materials::apply(&mut s, "steel_painted_worn").unwrap();
    let before = s.doc.project.walk(0).len();
    let bytes = s.smart_material_bytes(group, "My Steel").unwrap();
    execute(&mut s, "textureSet.select", &json!({"ts": 1})).unwrap();
    let id = s.add_smart_material_bytes(&bytes).unwrap();
    assert_eq!(s.doc.project.layer(id).unwrap().name, "My Steel");
    let walked = s.doc.project.walk(1).len();
    assert_eq!(walked, 1 + (before - 1), "the folder and its children arrive");
    assert!(s.add_smart_material_bytes(b"junk").is_err());
    s.doc.project.validate().unwrap();
}
