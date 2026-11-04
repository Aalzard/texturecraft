//! Multi-selection in the layer stack: toggle, range, and the actions on several layers, each one
//! undo step.

use serde_json::{Value, json};
use texturecraft_engine::core::{BlendMode, Channel, LayerId};
use texturecraft_engine::geom::primitives;
use texturecraft_engine::{Session, execute};

fn session() -> Session {
    Session::new_project("t", primitives::sphere(1.0, 16, 8), "builtin:sphere", None, 128).unwrap()
}

fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    execute(s, cmd, &p).unwrap_or_else(|e| panic!("{cmd}: {e}"))
}

/// Three fill layers, bottom to top; the top one is selected.
fn three(s: &mut Session) -> [LayerId; 3] {
    let mut ids = [LayerId(0); 3];
    for (i, id) in ids.iter_mut().enumerate() {
        *id = LayerId(run(s, "layer.add.fill", json!({"name": format!("L{i}")}))["layer"].as_u64().unwrap());
    }
    ids
}

fn sel(s: &Session) -> Vec<u64> {
    s.selection().iter().map(|l| l.0).collect()
}

/// The stack without the layer a new project starts with (ours are named L0, L1, ...).
fn stack(s: &Session) -> Vec<LayerId> {
    s.doc.project.texture_sets[0].stack.iter().copied().filter(|l| s.doc.project.layer(*l).unwrap().name.len() == 2 && s.doc.project.layer(*l).unwrap().name.starts_with('L')).collect()
}

#[test]
fn single_select_is_unchanged() {
    let mut s = session();
    let [a, _, c] = three(&mut s);
    assert_eq!(s.selected_layer(), Some(c));
    assert_eq!(sel(&s), vec![c.0]);
    run(&mut s, "layer.select", json!({"layer": a.0}));
    assert_eq!(sel(&s), vec![a.0]);
    assert_eq!(s.selected_layer(), Some(a));
}

#[test]
fn toggle_and_range() {
    let mut s = session();
    let [a, b, c] = three(&mut s);
    run(&mut s, "layer.select", json!({"layer": a.0}));
    run(&mut s, "layer.select", json!({"layer": c.0, "mode": "toggle"}));
    assert_eq!(sel(&s), vec![a.0, c.0]);
    // The toggled-in layer is the painting target.
    assert_eq!(s.selected_layer(), Some(c));
    // Toggling a selected layer takes it out; the other becomes the primary one.
    run(&mut s, "layer.select", json!({"layer": c.0, "mode": "toggle"}));
    assert_eq!(sel(&s), vec![a.0]);
    assert_eq!(s.selected_layer(), Some(a));
    // The last layer cannot be toggled away.
    run(&mut s, "layer.select", json!({"layer": a.0, "mode": "toggle"}));
    assert_eq!(sel(&s), vec![a.0]);
    // A range keeps its anchor as the primary layer.
    run(&mut s, "layer.select", json!({"layer": c.0, "mode": "range"}));
    assert_eq!(sel(&s), vec![a.0, b.0, c.0]);
    assert_eq!(s.selected_layer(), Some(a));
    // A plain select ends the multi-selection.
    run(&mut s, "layer.select", json!({"layer": b.0}));
    assert_eq!(sel(&s), vec![b.0]);
    let inspect = texturecraft_engine::inspect::session(&s);
    assert_eq!(inspect["selectedLayers"], json!([b.0]));
}

#[test]
fn selection_ends_when_another_command_selects() {
    let mut s = session();
    let [a, _, c] = three(&mut s);
    run(&mut s, "layer.select", json!({"layer": a.0}));
    run(&mut s, "layer.select", json!({"layer": c.0, "mode": "range"}));
    assert_eq!(sel(&s).len(), 3);
    run(&mut s, "layer.add.fill", json!({}));
    assert_eq!(sel(&s).len(), 1);
}

#[test]
fn remove_is_one_undo_step() {
    let mut s = session();
    let [a, b, c] = three(&mut s);
    run(&mut s, "layer.select", json!({"layer": a.0}));
    run(&mut s, "layer.select", json!({"layer": c.0, "mode": "toggle"}));
    let depth = s.history_labels().len();
    run(&mut s, "layer.remove", json!({}));
    assert_eq!(stack(&s), vec![b]);
    assert_eq!(s.selected_layer(), Some(b));
    assert_eq!(s.history_labels().len(), depth + 1);
    s.undo().unwrap();
    assert_eq!(stack(&s), vec![a, b, c]);
}

#[test]
fn duplicate_and_group() {
    let mut s = session();
    let [a, b, c] = three(&mut s);
    run(&mut s, "layer.select", json!({"layer": a.0}));
    run(&mut s, "layer.select", json!({"layer": c.0, "mode": "toggle"}));
    let depth = s.history_labels().len();
    let r = run(&mut s, "layer.duplicate", json!({}));
    assert_eq!(r["layers"].as_array().unwrap().len(), 2);
    assert_eq!(s.history_labels().len(), depth + 1);
    assert_eq!(s.doc.project.texture_sets[0].stack.len(), 6);
    assert_eq!(sel(&s).len(), 2);
    s.undo().unwrap();
    assert_eq!(stack(&s), vec![a, b, c]);

    run(&mut s, "layer.select", json!({"layer": a.0}));
    run(&mut s, "layer.select", json!({"layer": b.0, "mode": "toggle"}));
    let depth = s.history_labels().len();
    let g = LayerId(run(&mut s, "layer.group", json!({}))["layer"].as_u64().unwrap());
    assert_eq!(s.history_labels().len(), depth + 1);
    assert_eq!(s.doc.project.texture_sets[0].stack[1..], [g, c]);
    assert_eq!(s.doc.project.layer(g).unwrap().children(), &[a, b]);
    assert_eq!(sel(&s), vec![g.0]);
    s.undo().unwrap();
    assert_eq!(stack(&s), vec![a, b, c]);
}

#[test]
fn visibility_and_blend_for_all() {
    let mut s = session();
    let [a, b, c] = three(&mut s);
    run(&mut s, "layer.select", json!({"layer": a.0}));
    run(&mut s, "layer.select", json!({"layer": c.0, "mode": "range"}));
    let depth = s.history_labels().len();
    run(&mut s, "layer.visible", json!({}));
    assert!([a, b, c].iter().all(|l| !s.doc.project.layer(*l).unwrap().visible));
    // Mixed visibility shows everything.
    s.doc.project.layer_mut(b).unwrap().visible = true;
    run(&mut s, "layer.visible", json!({}));
    assert!([a, b, c].iter().all(|l| s.doc.project.layer(*l).unwrap().visible));
    run(&mut s, "layer.blend", json!({"channel": "baseColor", "opacity": 0.4, "mode": "multiply"}));
    for l in [a, b, c] {
        let ch = s.doc.project.layer(l).unwrap().channels[&Channel::BaseColor];
        assert_eq!(ch.opacity, 0.4);
        assert_eq!(ch.mode, BlendMode::Multiply);
    }
    assert_eq!(s.history_labels().len(), depth + 3);
    s.undo().unwrap();
    assert_eq!(s.doc.project.layer(a).unwrap().channels[&Channel::BaseColor].opacity, 1.0);
}

#[test]
fn explicit_layers_param_and_errors() {
    let mut s = session();
    let [a, b, c] = three(&mut s);
    run(&mut s, "layer.visible", json!({"layers": [a.0, b.0], "visible": false}));
    assert!(!s.doc.project.layer(a).unwrap().visible && !s.doc.project.layer(b).unwrap().visible);
    assert!(s.doc.project.layer(c).unwrap().visible);
    assert!(execute(&mut s, "layer.remove", &json!({"layers": [999]})).is_err());
    assert!(execute(&mut s, "layer.select", &json!({"layer": a.0, "mode": "nope"})).is_err());
    assert_eq!(stack(&s).len(), 3);
    run(&mut s, "layer.select", json!({"layers": [a.0, b.0]}));
    assert_eq!(sel(&s), vec![a.0, b.0]);
}
