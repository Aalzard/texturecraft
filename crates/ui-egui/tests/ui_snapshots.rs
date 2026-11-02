//! Offscreen pictures of the whole window, for checking layout and design without a visible
//! screen (locked machines, CI, agents):
//!
//! ```sh
//! TC_UI_SNAPSHOTS=<dir> cargo test -p texturecraft-ui-egui --release --test ui_snapshots -- --ignored
//! ```
//!
//! Writes `<dir>/<name>.png` at 1600x960: the default layout in each brightness, the strip as the
//! app-drawn title bar (Windows) and with the macOS traffic lights' inset, a floating panel and the
//! main windows. Ignored by default: it needs a GPU adapter and takes a few
//! seconds.

use std::path::{Path, PathBuf};

use egui_kittest::Harness;
use texturecraft_ui_egui::TextureCraftApp;
use texturecraft_ui_egui::docking::Panel;
use texturecraft_ui_egui::theme::Brightness;
use texturecraft_ui_egui::titlebar::TitleBar;

fn save(h: &mut Harness<'_, TextureCraftApp>, dir: &Path, name: &str) {
    h.run_steps(6);
    let img = h.render().unwrap_or_else(|e| panic!("{name}: {e}"));
    let path = dir.join(format!("{name}.png"));
    img.save(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    eprintln!("wrote {}", path.display());
}

#[test]
#[ignore]
fn window_snapshots() {
    let Some(dir) = std::env::var_os("TC_UI_SNAPSHOTS").map(PathBuf::from) else {
        eprintln!("set TC_UI_SNAPSHOTS=<dir> to write the pictures");
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let session = texturecraft_engine::sample_session(1024, 256).unwrap();
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 960.0)).with_pixels_per_point(1.0).wgpu().build_eframe(|cc| TextureCraftApp::new(cc, session));
    for (b, name) in [(Brightness::MediumDark, "main"), (Brightness::Dark, "brightness-dark"), (Brightness::Light, "brightness-light")] {
        h.state_mut().ui.prefs.brightness = b;
        save(&mut h, &dir, name);
    }
    h.state_mut().ui.prefs.brightness = Brightness::MediumDark;
    h.state_mut().title_bar = TitleBar::Integrated;
    save(&mut h, &dir, "titlebar-macos");
    h.state_mut().title_bar = TitleBar::App(craft_window::Layout::default());
    save(&mut h, &dir, "titlebar-app");
    // The close button under the pointer, and the glyphs at a fractional scale.
    h.hover_at(egui::pos2(1600.0 - 23.0, 14.0));
    save(&mut h, &dir, "titlebar-app-close-hover");
    h.hover_at(egui::pos2(800.0, 500.0));
    h.set_pixels_per_point(1.5);
    save(&mut h, &dir, "titlebar-app-150");
    h.set_pixels_per_point(1.0);
    h.state_mut().title_bar = TitleBar::System;
    h.state_mut().ui.dock.focus(Panel::Display);
    save(&mut h, &dir, "panel-display");
    h.state_mut().ui.dock.close(Panel::Display);
    h.state_mut().ui.windows.export = true;
    save(&mut h, &dir, "window-export");
    h.state_mut().ui.windows.export = false;
    h.state_mut().ui.windows.settings = true;
    save(&mut h, &dir, "window-settings");
    h.state_mut().ui.windows.settings = false;
    // A mask's levels effect over clouds: the histogram in Properties.
    let s = &mut h.state_mut().session;
    for (id, p) in [
        ("layer.add.fill", serde_json::json!({"color": [0.8, 0.3, 0.1]})),
        ("mask.add.black", serde_json::Value::Null),
        ("effect.add.fill", serde_json::json!({"target": "mask", "procedural": "clouds"})),
        ("effect.add.levels", serde_json::json!({"target": "mask", "inLow": 0.3, "inHigh": 0.8})),
    ] {
        texturecraft_engine::execute(s, id, &p).unwrap();
    }
    save(&mut h, &dir, "properties-levels");
}
