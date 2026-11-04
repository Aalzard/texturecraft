//! Importing SubCraft smart materials (`.scsmart`): the control command, undo, placement, the
//! report, a `.tcmat` round trip of the mapped stack, and files that must not hurt the session.
//! The fixtures are built from the public format description (`formats::scsmart::fixture`).

use std::path::PathBuf;

use serde_json::{Value, json};
use texturecraft_engine::core::{Channel, LayerId, LayerKind};
use texturecraft_engine::formats::scsmart::fixture::{self, Builder};
use texturecraft_engine::geom::primitives;
use texturecraft_engine::{Session, command_specs, execute};

fn session(res: u32) -> Session {
    let mut s = Session::new_project("t", primitives::sphere(1.0, 48, 24), "builtin:sphere", None, res).unwrap();
    s.viewport = texturecraft_engine::geom::glam::Vec2::new(400.0, 400.0);
    s.update();
    s
}

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tc_scsmart_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &std::path::Path, name: &str, bytes: &[u8]) -> String {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p.display().to_string()
}

fn names(s: &Session) -> Vec<String> {
    s.doc.project.walk(0).iter().map(|(id, depth)| format!("{}{}", "  ".repeat(*depth), s.doc.project.layers[id].name)).collect()
}

fn layer_named(s: &Session, name: &str) -> LayerId {
    *s.doc.project.layers.iter().find(|(_, l)| l.name == name).map(|(id, _)| id).unwrap()
}

#[test]
fn imports_a_smart_material_as_one_undo_step_with_a_report() {
    let dir = scratch_dir("import");
    let path = write(&dir, "Painted Metal.scsmart", &fixture::sample());
    let mut s = session(256);
    let before = names(&s);
    let r = execute(&mut s, "material.importScsmart", &json!({"path": path})).unwrap();
    assert_eq!(r["name"], "Painted Metal");
    assert_eq!(r["import"]["layers"], 4);
    assert_eq!(r["import"]["masks"], 1);
    let issues = r["import"]["issues"].as_array().unwrap();
    assert!(issues.iter().any(|i| i["severity"] == "dropped" && i["message"].as_str().unwrap().contains("baked maps")), "{issues:#?}");
    assert!(issues.iter().any(|i| i["severity"] == "approximated" && i["what"].as_str().unwrap().contains("wormhole")));
    let after = names(&s);
    assert_eq!(after.len(), before.len() + 4, "{after:?}");
    assert!(after.iter().any(|n| n == "Painted Metal") && after.iter().any(|n| n == "  Steel") && after.iter().any(|n| n == "  Scratches"));
    s.doc.project.validate().unwrap();

    // The pixels were resampled from the file's 128 to the set's 256.
    let scratches = layer_named(&s, "Scratches");
    assert_eq!(s.doc.pixels.layer(scratches, Channel::Roughness).unwrap().width(), 256);
    // The embedded images came along and the fills point at them.
    assert!(s.doc.resources.len() >= 2);
    let LayerKind::Fill(f) = &s.doc.project.layers[&layer_named(&s, "Steel")].kind else { panic!("fill") };
    assert!(matches!(f.sources.get(&Channel::Normal), Some(texturecraft_engine::core::Source::Image { name, .. }) if s.doc.resources.contains_key(name)));
    // The report went to the log.
    assert!(s.log.iter().any(|l| l.contains("SubCraft: Imported \"Painted Metal\"")), "{:?}", s.log);

    // It composites to the steel color and nothing is NaN anywhere.
    s.update();
    let v = s.caches[0].composite.value(Channel::BaseColor, 40, 40);
    assert!((v[0] - 0.6).abs() < 0.02 && (v[2] - 0.65).abs() < 0.02, "{v:?}");
    for ch in [Channel::BaseColor, Channel::Roughness, Channel::Metallic, Channel::Normal, Channel::Height] {
        for (x, y) in [(0, 0), (10, 200), (130, 130), (255, 255), (200, 20)] {
            assert!(s.caches[0].composite.value(ch, x, y).iter().all(|c| c.is_finite()), "{ch:?} at {x},{y}");
        }
    }

    // One undo step takes it all back.
    execute(&mut s, "edit.undo", &Value::Null).unwrap();
    assert_eq!(names(&s), before);
    execute(&mut s, "edit.redo", &Value::Null).unwrap();
    assert_eq!(names(&s), after);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn material_load_recognizes_scsmart_files_and_places_them() {
    let dir = scratch_dir("load");
    let path = write(&dir, "m.scsmart", &fixture::sample());
    let mut s = session(256);
    let base = s.selected_layer().unwrap();
    let r = execute(&mut s, "material.load", &json!({"path": path, "above": base.0})).unwrap();
    assert!(r["import"]["layers"].as_u64().unwrap() >= 4, "{r}");
    assert_eq!(s.doc.project.walk(0).len(), 5);
    // Importing again shares the images instead of duplicating them.
    let resources = s.doc.resources.len();
    execute(&mut s, "material.load", &json!({"path": path})).unwrap();
    assert_eq!(s.doc.resources.len(), resources);
    s.doc.project.validate().unwrap();
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_mapped_stack_survives_a_tcmat_round_trip() {
    let dir = scratch_dir("roundtrip");
    let path = write(&dir, "m.scsmart", &fixture::sample());
    let mut a = session(256);
    let r = execute(&mut a, "material.importScsmart", &json!({"path": path})).unwrap();
    let folder = r["layer"].as_u64().unwrap();
    let saved = dir.join("saved.tcmat").display().to_string();
    execute(&mut a, "material.save", &json!({"layer": folder, "path": saved})).unwrap();

    let mut b = session(256);
    execute(&mut b, "material.load", &json!({"path": saved})).unwrap();
    // The same tree: names, kinds, visibility, per-channel blend, masks and their effects.
    let describe = |s: &Session| -> Vec<Value> {
        s.doc
            .project
            .walk(0)
            .iter()
            .filter(|(id, _)| s.doc.project.layers[id].name != "Layer 1")
            .map(|(id, depth)| {
                let l = &s.doc.project.layers[id];
                json!({
                    "name": l.name, "depth": depth, "kind": l.kind.name(), "visible": l.visible,
                    "channels": l.channels.iter().map(|(c, b)| format!("{c:?}:{:?}:{}", b.mode, b.opacity)).collect::<Vec<_>>(),
                    "mask": l.mask.as_ref().map(|m| m.effects.iter().map(|e| (e.kind.name(), format!("{:?}", e.blend), e.opacity)).collect::<Vec<_>>()),
                    "content_effects": l.content_effects.len(),
                })
            })
            .collect()
    };
    assert_eq!(describe(&a), describe(&b));
    assert_eq!(a.doc.resources.keys().collect::<Vec<_>>(), b.doc.resources.keys().collect::<Vec<_>>());
    // Painted pixels are identical.
    let (sa, sb) = (layer_named(&a, "Scratches"), layer_named(&b, "Scratches"));
    for (x, y) in [(70, 5), (200, 9), (3, 3), (130, 130)] {
        assert_eq!(a.doc.pixels.layer(sa, Channel::Roughness).unwrap().get(x, y), b.doc.pixels.layer(sb, Channel::Roughness).unwrap().get(x, y));
    }
    // And the two composite the same.
    a.update();
    b.update();
    for ch in [Channel::BaseColor, Channel::Roughness, Channel::Metallic] {
        for (x, y) in [(40, 40), (150, 150), (220, 30)] {
            let (va, vb) = (a.caches[0].composite.value(ch, x, y), b.caches[0].composite.value(ch, x, y));
            assert!((0..3).all(|k| (va[k] - vb[k]).abs() < 1e-3), "{ch:?} {x},{y}: {va:?} {vb:?}");
        }
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn imports_inside_the_selected_folder() {
    let dir = scratch_dir("inside");
    let path = write(&dir, "m.scsmart", &fixture::sample());
    let mut s = session(256);
    execute(&mut s, "layer.group", &Value::Null).unwrap();
    let group = s.selected_layer().unwrap();
    execute(&mut s, "material.importScsmart", &json!({"path": path, "above": group.0, "inside": true})).unwrap();
    assert_eq!(s.doc.project.layers[&group].children().len(), 2, "the original layer plus the imported folder");
    s.doc.project.validate().unwrap();
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn files_that_are_not_smart_materials_leave_the_session_untouched() {
    let dir = scratch_dir("hostile");
    let good = fixture::sample();
    let mut mask = Builder::new("Dirt", 128);
    mask.json["kind"] = json!("smart_mask");
    let mut future =
        Builder::new("Future", 128).layers(json!([{"id": "a", "name": "A", "visible": true, "blend": [], "content": {"type": "paint", "channels": []}}]));
    future.json["schema_version"] = json!(99);
    let files: Vec<(&str, Vec<u8>)> = vec![
        ("empty.scsmart", Vec::new()),
        ("garbage.scsmart", b"this is not a zip".to_vec()),
        ("truncated.scsmart", good[..good.len() / 2].to_vec()),
        ("tail_cut.scsmart", good[..good.len() - 3].to_vec()),
        ("mask.scsmart", mask.build()),
        ("future.scsmart", future.build()),
        ("nolayers.scsmart", Builder::new("None", 128).build()),
    ];
    let mut s = session(256);
    let before = names(&s);
    for (name, bytes) in &files {
        let path = write(&dir, name, bytes);
        for cmd in ["material.importScsmart", "material.load"] {
            let e = execute(&mut s, cmd, &json!({"path": path})).expect_err(name);
            assert!(!e.to_string().is_empty());
        }
        assert_eq!(names(&s), before, "{name} must not change the project");
    }
    let e = execute(&mut s, "material.importScsmart", &json!({"path": dir.join("missing.scsmart").display().to_string()})).unwrap_err();
    assert!(e.to_string().contains("cannot read"), "{e}");
    assert!(execute(&mut s, "material.importScsmart", &json!({})).is_err());
    assert!(execute(&mut s, "material.importScsmart", &json!({"path": dir.display().to_string()})).is_err(), "a folder is not a file");
    // Only the history of the session before: nothing to undo.
    assert!(!s.can_undo() || execute(&mut s, "edit.undo", &Value::Null).is_ok());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_command_survives_the_panic_hunt_params() {
    let hostile = [
        Value::Null,
        json!({"path": ""}),
        json!({"path": "/dev/null"}),
        json!({"path": "../../etc/passwd", "above": u64::MAX, "inside": "yes", "index": -5}),
        json!({"path": 5, "above": {}}),
    ];
    assert!(command_specs().iter().any(|c| c.id == "material.importScsmart"));
    for p in &hostile {
        let mut s = session(256);
        let _ = execute(&mut s, "material.importScsmart", p);
        s.update();
    }
}
