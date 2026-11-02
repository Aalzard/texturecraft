//! Content effects, HSL and anchor points through engine commands.

use serde_json::{Value, json};
use texturecraft_engine::core::{Channel, LayerId};
use texturecraft_engine::geom::primitives;
use texturecraft_engine::{Session, StackTarget, execute};

fn session() -> Session {
    let mut s = Session::new_project("t", primitives::sphere(1.0, 48, 24), "builtin:sphere", None, 256).unwrap();
    s.viewport = texturecraft_engine::geom::glam::Vec2::new(400.0, 400.0);
    s.camera.yaw = 0.0;
    s.camera.pitch = 0.0;
    s.update();
    s
}

/// Composited value of `c` under the viewport center.
fn center(s: &mut Session, c: Channel) -> [f32; 3] {
    s.update();
    let hit = s.pick(texturecraft_engine::geom::glam::Vec2::new(200.0, 200.0), s.viewport).unwrap();
    s.caches[0].composite.value(c, (hit.uv.x * 256.0) as u32, (hit.uv.y * 256.0) as u32)
}

fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() < 2e-3)
}

fn id(v: Value, key: &str) -> u64 {
    v[key].as_u64().unwrap()
}

#[test]
fn content_effects_commands_inspect_and_round_trip() {
    let mut s = session();
    let layer = id(execute(&mut s, "layer.add.fill", &json!({"color": [1.0, 0.0, 0.0], "roughness": 0.2})).unwrap(), "layer");
    let hsl = id(execute(&mut s, "effect.add.filter", &json!({"target": "content", "filter": "hsl", "hue": 1.0 / 3.0})).unwrap(), "effect");
    assert!(close(center(&mut s, Channel::BaseColor), [0.0, 1.0, 0.0]), "{:?}", center(&mut s, Channel::BaseColor));
    assert!(s.doc.project.layer(LayerId(layer)).unwrap().mask.is_none(), "content effects don't create a mask");
    assert_eq!(s.target, StackTarget::Effect(LayerId(hsl)));
    // Mask-only effects refuse the content, unknown targets are errors.
    assert!(execute(&mut s, "effect.add.generator", &json!({"target": "content"})).is_err());
    assert!(execute(&mut s, "effect.add.filter", &json!({"target": "nope"})).is_err());
    execute(&mut s, "effect.set", &json!({"effect": hsl, "hue": 0.0, "lightness": -1.0})).unwrap();
    assert!(close(center(&mut s, Channel::BaseColor), [0.0; 3]));
    execute(&mut s, "effect.set", &json!({"effect": hsl, "visible": false})).unwrap();

    // A content fill effect, edited through layer.fill.set.
    let fill = id(execute(&mut s, "effect.add.fill", &json!({"target": "content", "roughness": 0.7})).unwrap(), "effect");
    assert!(close(center(&mut s, Channel::Roughness), [0.7; 3]));
    assert!(close(center(&mut s, Channel::BaseColor), [1.0, 0.0, 0.0]), "channels without a source stay");
    execute(&mut s, "layer.fill.set", &json!({"effect": fill, "channel": "roughness", "value": 0.9})).unwrap();
    assert!(close(center(&mut s, Channel::Roughness), [0.9; 3]));
    execute(&mut s, "layer.fill.set", &json!({"effect": fill, "channel": "roughness", "enabled": false})).unwrap();
    assert!(close(center(&mut s, Channel::Roughness), [0.2; 3]));
    assert!(execute(&mut s, "layer.fill.set", &json!({"effect": hsl, "channel": "roughness", "value": 0.5})).is_err(), "not a fill effect");

    // Inspect shows the content stack; the project survives a save/load round trip.
    let inspect = execute(&mut s, "project.inspect", &Value::Null).unwrap();
    let row = inspect["layers"].as_array().unwrap().iter().find(|l| l["id"] == layer).unwrap().clone();
    let kinds: Vec<&str> = row["contentEffects"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["HSL Perceptive", "Fill"], "{row}");
    let back = Session::from_project_bytes(&s.to_project_bytes().unwrap()).unwrap();
    assert_eq!(back.doc.project, s.doc.project);

    execute(&mut s, "layer.select", &json!({"layer": layer, "target": fill})).unwrap();
    execute(&mut s, "effect.remove", &json!({"effect": fill})).unwrap();
    assert_eq!(s.target, StackTarget::Content);
    execute(&mut s, "edit.undo", &Value::Null).unwrap();
    assert_eq!(s.doc.project.layer(LayerId(layer)).unwrap().content_effects.len(), 2);
}

#[test]
fn anchor_points_feed_masks_above() {
    let mut s = session();
    let low = id(execute(&mut s, "layer.add.fill", &json!({"height": 0.5})).unwrap(), "layer");
    let anchor = id(execute(&mut s, "effect.add.anchor", &json!({"name": "Height"})).unwrap(), "effect");
    assert_eq!(s.doc.project.effect(LayerId(anchor)).unwrap().1.name, "Height");
    execute(&mut s, "layer.add.fill", &json!({"color": [1.0, 1.0, 1.0]})).unwrap();
    // A layer cannot read an anchor on itself or above it.
    assert!(execute(&mut s, "effect.add.fill", &json!({"layer": low, "anchor": anchor})).is_err());
    assert!(execute(&mut s, "effect.add.fill", &json!({"anchor": low})).is_err(), "not an anchor point");
    execute(&mut s, "mask.add.black", &Value::Null).unwrap();
    let reader = id(execute(&mut s, "effect.add.fill", &json!({"anchor": anchor, "anchorChannel": "height"})).unwrap(), "effect");
    // Base color: the low fill's default gray, then white through a 0.5 mask.
    assert!(close(center(&mut s, Channel::BaseColor), [0.75; 3]), "{:?}", center(&mut s, Channel::BaseColor));
    // Reading the anchor's base color as gray instead.
    execute(&mut s, "effect.set", &json!({"effect": reader, "anchorChannel": "baseColor"})).unwrap();
    assert!(close(center(&mut s, Channel::BaseColor), [0.75; 3]));
    execute(&mut s, "effect.set", &json!({"effect": reader, "value": 1.0})).unwrap();
    assert!(close(center(&mut s, Channel::BaseColor), [1.0; 3]), "back to a uniform fill");
    // A blurred mask still reads the anchor (a flat height stays flat).
    execute(&mut s, "effect.set", &json!({"effect": reader, "anchor": anchor})).unwrap();
    execute(&mut s, "effect.add.filter", &json!({"filter": "blur", "radius": 8})).unwrap();
    assert!(close(center(&mut s, Channel::BaseColor), [0.75; 3]));
    let inspect = execute(&mut s, "project.inspect", &Value::Null).unwrap();
    assert!(inspect.to_string().contains(&format!("\"anchor\":{anchor}")), "{inspect}");
}

#[test]
fn painting_into_a_content_paint_effect() {
    let mut s = session();
    execute(&mut s, "layer.add.fill", &json!({"roughness": 0.3})).unwrap();
    let e = id(execute(&mut s, "effect.add.paint", &json!({"target": "content"})).unwrap(), "effect");
    execute(&mut s, "tool.material", &json!({"channel": "roughness", "value": 0.9})).unwrap();
    execute(&mut s, "tool.brush", &json!({"size": 20, "hardness": 90})).unwrap();
    execute(&mut s, "tool.stroke", &json!({"points": [[180, 200], [220, 200]]})).unwrap();
    assert!(s.doc.pixels.layer(LayerId(e), Channel::Roughness).is_some());
    assert!(center(&mut s, Channel::Roughness)[0] > 0.85, "{:?}", center(&mut s, Channel::Roughness));
    // Flattening bakes the effect into a paint layer and drops its pixels.
    execute(&mut s, "layer.flatten", &Value::Null).unwrap();
    assert!(s.doc.pixels.layer(LayerId(e), Channel::Roughness).is_none());
    assert!(center(&mut s, Channel::Roughness)[0] > 0.85);
}

#[test]
fn histogram_counts_what_an_effect_reads() {
    let mut s = session();
    let hist = |s: &mut Session, p: Value| {
        let r = execute(s, "effect.histogram", &p).unwrap();
        let bins: Vec<u64> = r["bins"].as_array().unwrap().iter().map(|b| b.as_u64().unwrap()).collect();
        (r["target"].as_str().unwrap().to_string(), bins)
    };
    // Only the bucket holding the value is filled.
    let one = |bins: &[u64], k: usize| bins.iter().enumerate().all(|(i, b)| (*b > 0) == (i == k));
    execute(&mut s, "layer.add.fill", &json!({"color": [0.25, 0.25, 0.25], "roughness": 0.75})).unwrap();
    let lv = id(execute(&mut s, "effect.add.levels", &json!({"target": "content", "inLow": 0.5})).unwrap(), "effect");
    execute(&mut s, "effect.add.filter", &json!({"target": "content", "filter": "invert"})).unwrap();
    let (target, bins) = hist(&mut s, json!({"effect": lv, "channel": "roughness", "bins": 4}));
    assert_eq!(target, "content");
    assert!(one(&bins, 3), "roughness 0.75: {bins:?}");
    // The effect itself and those above it are left out; the default channel is the first.
    let (_, bins) = hist(&mut s, json!({"effect": lv, "bins": 4}));
    assert!(one(&bins, 1), "base color 0.25, components pooled: {bins:?}");
    assert_eq!(bins.iter().sum::<u64>() % 3, 0);
    // A mask's levels reads the mask below it.
    execute(&mut s, "mask.add.black", &Value::Null).unwrap();
    execute(&mut s, "effect.add.fill", &json!({"target": "mask", "value": 0.3})).unwrap();
    let ml = id(execute(&mut s, "effect.add.levels", &json!({"target": "mask"})).unwrap(), "effect");
    let (target, bins) = hist(&mut s, json!({"effect": ml, "bins": 4}));
    assert_eq!(target, "mask");
    assert!(one(&bins, 1), "mask 0.3: {bins:?}");
    assert_eq!(hist(&mut s, json!({"effect": ml})).1.len(), 64);
    // Below a blur, every tile builds its own neighbourhood field; the texels counted stay the same.
    let texels: u64 = bins.iter().sum();
    execute(&mut s, "effect.remove", &json!({"effect": ml})).unwrap();
    execute(&mut s, "effect.add.filter", &json!({"target": "mask", "filter": "blur", "radius": 4})).unwrap();
    let ml = id(execute(&mut s, "effect.add.levels", &json!({"target": "mask"})).unwrap(), "effect");
    let (_, blurred) = hist(&mut s, json!({"effect": ml, "bins": 4}));
    assert_eq!(blurred.iter().sum::<u64>(), texels);
    assert_eq!(blurred.iter().enumerate().max_by_key(|(_, b)| **b).map(|(i, _)| i), Some(1), "{blurred:?}");
    // A paint layer with nothing painted yet still has its mask.
    execute(&mut s, "layer.add.paint", &Value::Null).unwrap();
    execute(&mut s, "mask.add.black", &Value::Null).unwrap();
    execute(&mut s, "effect.add.fill", &json!({"target": "mask", "value": 0.6})).unwrap();
    let pl = id(execute(&mut s, "effect.add.levels", &json!({"target": "mask"})).unwrap(), "effect");
    assert!(one(&hist(&mut s, json!({"effect": pl, "bins": 4})).1, 2), "an empty paint layer's mask");
    assert!(execute(&mut s, "effect.histogram", &json!({"effect": 999_999})).is_err());
    assert!(execute(&mut s, "effect.histogram", &json!({"effect": lv, "channel": "nope"})).is_err());
}
