//! The sliders of a selected effect (opacity, generator, levels…) apply what they were dragged
//! or clicked to.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use serde_json::json;
use texturecraft_engine::core::{EffectKind, LayerId};
use texturecraft_ui_egui::TextureCraftApp;

/// The sample session with its first generator effect selected, and that effect's id.
fn harness() -> (Harness<'static, TextureCraftApp>, LayerId) {
    let mut session = texturecraft_engine::sample_session(256, 64).unwrap();
    let (layer, effect) = session
        .doc
        .project
        .layers
        .values()
        .find_map(|l| {
            let mask = l.mask.iter().flat_map(|m| m.effects.iter());
            l.content_effects.iter().chain(mask).find(|e| matches!(e.kind, EffectKind::Generator(_))).map(|e| (l.id, e.id))
        })
        .unwrap();
    texturecraft_engine::execute(&mut session, "layer.select", &json!({"layer": layer.0, "target": effect.0})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1400.0)).build_eframe(|cc| TextureCraftApp::new(cc, session));
    h.run_steps(4);
    (h, effect)
}

fn opacity(h: &Harness<'_, TextureCraftApp>, effect: LayerId) -> f32 {
    h.state().session.doc.project.effect(effect).unwrap().1.opacity
}

/// The track under the slider labelled `label` (the row below the label).
fn track(h: &Harness<'_, TextureCraftApp>, label: &str) -> egui::Rect {
    let l = h.get_by_label(label).rect();
    egui::Rect::from_min_max(egui::pos2(l.left() + 6.0, l.bottom() + 6.0), egui::pos2(l.left() + 200.0, l.bottom() + 14.0))
}

#[test]
fn dragging_the_effect_opacity_applies_it() {
    let (mut h, effect) = harness();
    assert_eq!(opacity(&h, effect), 1.0);
    let t = track(&h, "Opacity");
    h.drag_at(egui::pos2(t.right(), t.center().y));
    h.run_steps(2);
    h.hover_at(egui::pos2(t.left() + 20.0, t.center().y));
    h.run_steps(2);
    h.drop_at(egui::pos2(t.left() + 20.0, t.center().y));
    h.run_steps(3);
    assert!(opacity(&h, effect) < 0.5, "the dragged opacity applies: {}", opacity(&h, effect));
}

/// The combo box in the row of the "Channel" label.
fn channel_picker<'a>(h: &'a Harness<'_, TextureCraftApp>) -> egui_kittest::Node<'a> {
    let row = h.get_by_label("Channel").rect().center().y;
    h.get_all(by().role(Role::ComboBox)).find(|n| (n.rect().center().y - row).abs() < 4.0).unwrap()
}

/// A content levels effect offers a channel for its histogram; picking one keeps it selected.
#[test]
fn content_levels_pick_the_histogram_channel() {
    let mut session = texturecraft_engine::sample_session(256, 64).unwrap();
    texturecraft_engine::execute(&mut session, "layer.add.fill", &json!({"color": [0.5, 0.2, 0.1], "roughness": 0.6})).unwrap();
    texturecraft_engine::execute(&mut session, "effect.add.levels", &json!({"target": "content"})).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1400.0)).build_eframe(|cc| TextureCraftApp::new(cc, session));
    h.run_steps(4);
    assert_eq!(channel_picker(&h).value().as_deref(), Some("Base color"), "the layer's first channel");
    channel_picker(&h).click();
    h.run_steps(2);
    h.get_by_label("Roughness").click();
    h.run_steps(3);
    assert_eq!(channel_picker(&h).value().as_deref(), Some("Roughness"), "the picked channel stays");
}

#[test]
fn clicking_the_effect_opacity_applies_it() {
    let (mut h, effect) = harness();
    let t = track(&h, "Opacity");
    let at = egui::pos2(t.left() + 20.0, t.center().y);
    h.drag_at(at);
    h.drop_at(at);
    h.run_steps(3);
    assert!(opacity(&h, effect) < 0.5, "a click on the track applies: {}", opacity(&h, effect));
}
