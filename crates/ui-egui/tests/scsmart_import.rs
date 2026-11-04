//! Importing a SubCraft smart material from the UI: the summary window opens when something did
//! not map exactly, a clean import only toasts, and the File menu offers the import.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use texturecraft_engine::Session;
use texturecraft_engine::formats::scsmart::fixture::{self, Builder};
use texturecraft_engine::geom::primitives;
use texturecraft_ui_egui::TextureCraftApp;

fn harness() -> Harness<'static, TextureCraftApp> {
    let session = Session::new_project("t", primitives::sphere(1.0, 24, 12), "builtin:sphere", None, 128).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 960.0)).build_eframe(|cc| TextureCraftApp::new(cc, session));
    h.run_steps(4);
    h
}

fn file(name: &str, bytes: &[u8]) -> String {
    let dir = std::env::temp_dir().join(format!("tc_ui_scsmart_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p.display().to_string()
}

#[test]
fn an_import_with_differences_opens_the_summary() {
    let mut h = harness();
    let path = file("Painted Metal.scsmart", &fixture::sample());
    let layers = h.state().session.doc.project.layers.len();
    assert!(h.state_mut().run("material.load", json!({"path": path})).is_some());
    h.run_steps(3);
    assert!(h.state().session.doc.project.layers.len() > layers, "the layers were added");
    assert!(h.state().ui.windows.import_summary.is_some(), "approximated and dropped things open the summary");
    h.get_by_label("SubCraft import");
    h.get_by_label("Painted Metal");
    h.get_by_label("Not imported");
    h.get_by_label("Approximated");
    assert!(h.state().toasts.iter().any(|t| t.text.contains("differences")), "{:?}", h.state().toasts.iter().map(|t| &t.text).collect::<Vec<_>>());
    h.get_by_label("Close").click();
    h.run_steps(3);
    assert!(h.state().ui.windows.import_summary.is_none(), "Close dismisses the summary");
}

#[test]
fn a_clean_import_only_toasts() {
    let mut h = harness();
    let layer = json!({"id": "a", "name": "Plain", "visible": true, "blend": [{"channel": "base_color", "mode": "normal", "opacity": 1.0}],
        "content": {"type": "fill", "projection": {"mode": "uv"}, "channels": [{"channel": "base_color", "enabled": true, "source": {"type": "uniform", "value": [0.2, 0.4, 0.6]}}]}});
    let path = file("Plain.scsmart", &Builder::new("Plain", 128).layers(json!([layer])).build());
    h.state_mut().run("material.importScsmart", json!({"path": path})).unwrap();
    h.run_steps(3);
    assert!(h.state().ui.windows.import_summary.is_none());
    assert!(h.state().toasts.iter().any(|t| t.text.contains("Plain") && !t.error));
}

#[test]
fn a_broken_file_is_a_toast_and_changes_nothing() {
    let mut h = harness();
    let layers = h.state().session.doc.project.layers.len();
    let path = file("broken.scsmart", b"not a zip");
    assert!(h.state_mut().run("material.importScsmart", json!({"path": path})).is_none());
    h.run_steps(3);
    assert_eq!(h.state().session.doc.project.layers.len(), layers);
    assert!(h.state().toasts.iter().any(|t| t.error));
    assert!(h.state().ui.windows.import_summary.is_none());
}

#[test]
fn the_file_menu_lists_the_import() {
    assert!(texturecraft_ui_egui::menus::CATALOG.iter().any(|i| i.menu == "File" && i.id == "material.importScsmart"));
    assert!(texturecraft_engine::find("material.importScsmart").is_some(), "an item is live when its command exists");
}
