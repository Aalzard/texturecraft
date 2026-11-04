//! The Preview size sliders: Assets tiles per row follow the tile size, Ctrl+wheel over the grid
//! changes it, and the Layers row height follows the thumbnail size.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use texturecraft_ui_egui::TextureCraftApp;

fn harness() -> Harness<'static, TextureCraftApp> {
    let session = texturecraft_engine::sample_session(256, 64).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 960.0)).build_eframe(|cc| TextureCraftApp::new(cc, session));
    h.run_steps(4);
    h
}

/// The rects of the Assets tiles (buttons as wide as `width`).
fn tiles(h: &Harness<'_, TextureCraftApp>, width: f32) -> Vec<egui::Rect> {
    h.get_all_by(|n| n.role() == Role::Button).map(|n| n.rect()).filter(|r| (r.width() - width).abs() < 0.6 && r.height() > width).collect()
}

fn per_row(tiles: &[egui::Rect]) -> usize {
    tiles.first().map_or(0, |f| tiles.iter().filter(|t| (t.top() - f.top()).abs() < 1.0).count())
}

#[test]
fn smaller_tiles_fit_more_per_row() {
    let mut h = harness();
    let default = tiles(&h, 72.0);
    assert!(!default.is_empty(), "the default tile is 72 px wide");
    let mut counts = Vec::new();
    for size in [48.0, 72.0, 160.0] {
        h.state_mut().ui.prefs.shelf_tile_size = size;
        h.run_steps(3);
        let t = tiles(&h, size);
        assert!(!t.is_empty(), "tiles are {size} px wide");
        counts.push(per_row(&t));
    }
    assert!(counts[0] > counts[1] && counts[1] > counts[2], "{counts:?}");
}

#[test]
fn ctrl_wheel_over_the_grid_changes_the_tile_size() {
    let mut h = harness();
    let at = tiles(&h, 72.0)[0].center();
    h.hover_at(at);
    h.run_steps(2);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, 60.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers: egui::Modifiers::COMMAND,
    });
    h.run_steps(3);
    let bigger = h.state().ui.prefs.shelf_tile_size;
    assert!(bigger > 72.0 && bigger <= 160.0, "{bigger}");
}

#[test]
fn the_layer_row_height_follows_the_preview_size() {
    let mut h = harness();
    let name = {
        let s = &h.state().session;
        s.doc.project.layers[&s.selected_layer().unwrap()].name.clone()
    };
    let row = |h: &Harness<'_, TextureCraftApp>| h.get_by(|n| n.role() == Role::ListItem && n.label().as_deref() == Some(name.as_str())).rect().height();
    assert_eq!(row(&h), 38.0, "the default row is unchanged");
    h.state_mut().ui.prefs.layer_preview_size = 64.0;
    h.run_steps(3);
    assert_eq!(row(&h), 74.0);
    h.state_mut().ui.prefs.layer_preview_size = 1.0; // clamped to 24, the row has a floor
    h.run_steps(3);
    assert_eq!(row(&h), 34.0);
}
