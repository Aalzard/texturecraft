//! Pen pressure (touch force) thins strokes in the 2D view as it does in the 3D view.

use egui::{Event, Modifiers, PointerButton, Pos2, TouchDeviceId, TouchId, TouchPhase, pos2};
use egui_kittest::Harness;
use texturecraft_engine::core::Channel;
use texturecraft_ui_egui::TextureCraftApp;
use texturecraft_ui_egui::state::ViewLayout;

/// One frame of pen input: the windowing layer sends a pen sample as a touch with force plus the
/// pointer events it drives, in the same frame (None: a mouse, which reports no force).
fn frame(h: &mut Harness<'_, TextureCraftApp>, force: Option<f32>, pos: Pos2, phase: TouchPhase, button: Option<bool>) {
    let events = &mut h.input_mut().events;
    if let Some(force) = force {
        events.push(Event::Touch { device_id: TouchDeviceId(1), id: TouchId(1), phase, pos, force: Some(force) });
    }
    events.push(Event::PointerMoved(pos));
    if let Some(pressed) = button {
        events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.step();
}

/// Paint one stroke across the 2D view and return the summed base color alpha it added.
fn stroke_2d(force: Option<f32>) -> f64 {
    let session = texturecraft_engine::sample_session(256, 64).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 1500.0)).build_eframe(|cc| TextureCraftApp::new(cc, session));
    h.state_mut().ui.layout = ViewLayout::Only2D;
    h.state_mut().session.tool.brush.min_size = 0.0;
    h.run_steps(4);
    let before = painted(&h);
    let (from, to) = (pos2(600.0, 500.0), pos2(800.0, 500.0));
    frame(&mut h, force, from, TouchPhase::Start, Some(true));
    for k in 1..=8 {
        frame(&mut h, force, from + (to - from) * (k as f32 / 8.0), TouchPhase::Move, None);
    }
    frame(&mut h, force, to, TouchPhase::End, Some(false));
    h.run_steps(3);
    painted(&h) - before
}

/// The selected layer's summed base color alpha.
fn painted(h: &Harness<'_, TextureCraftApp>) -> f64 {
    let s = &h.state().session;
    let id = s.selected_layer().expect("a selected layer");
    let Some(img) = s.doc.pixels.layer(id, Channel::BaseColor) else { return 0.0 };
    let (w, hgt) = s.doc.project.texture_sets[s.ts()].size();
    (0..hgt).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| img.get(x, y)[3] as f64 / 65535.0).sum()
}

#[test]
fn pen_pressure_thins_strokes_in_the_2d_view() {
    let mouse = stroke_2d(None);
    let light = stroke_2d(Some(0.2));
    assert!(mouse > 10.0, "the mouse stroke painted: {mouse}");
    assert!(light > 0.0 && light < mouse * 0.5, "a light pen paints a thinner stroke: {light} vs {mouse}");
}
