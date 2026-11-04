//! The viewport bar's mirror button: its popover edits the brush symmetry, stays open while
//! you change its controls and closes on a click outside; X toggles mirror X.

use egui::Key;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use texturecraft_ui_egui::TextureCraftApp;

const BUTTON: &str = "Symmetry and lazy mouse (X)";

fn app() -> Harness<'static, TextureCraftApp> {
    let session = texturecraft_engine::sample_session(256, 64).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 960.0)).build_eframe(|cc| TextureCraftApp::new(cc, session));
    h.run_steps(4);
    h
}

#[test]
fn popover_edits_symmetry_and_closes_on_click_outside() {
    let mut h = app();
    assert!(h.query_by_label("Show symmetry guides").is_none(), "closed at start");
    h.get_by_label(BUTTON).click();
    h.run_steps(3);
    // Mirror X, then Y: both stay on (any combination), and the popover stays open.
    h.get_all_by_label("X").next().unwrap().click();
    h.run_steps(2);
    h.get_all_by_label("Y").next().unwrap().click();
    h.run_steps(2);
    let sym = h.state().session.tool.brush.symmetry;
    assert!(sym.x && sym.y && !sym.z, "{sym:?}");
    assert!(h.state().session.tool.show_symmetry_guides);
    h.get_by_label("Show symmetry guides").click();
    h.run_steps(2);
    assert!(!h.state().session.tool.show_symmetry_guides);
    assert!(h.query_by_label("Show symmetry guides").is_some(), "still open while editing");
    // A click on the status bar, outside the popover, closes it.
    let p = egui::pos2(300.0, 960.0 - 10.0);
    h.drag_at(p);
    h.drop_at(p);
    h.run_steps(3);
    assert!(h.query_by_label("Show symmetry guides").is_none(), "closed by the click outside");
}

#[test]
fn x_key_toggles_mirror_x() {
    let mut h = app();
    assert!(!h.state().session.tool.brush.symmetry.x);
    h.key_press(Key::X);
    h.run_steps(2);
    assert!(h.state().session.tool.brush.symmetry.x, "X turns mirror X on");
    h.key_press(Key::X);
    h.run_steps(2);
    assert!(!h.state().session.tool.brush.symmetry.x, "and off again");
}
