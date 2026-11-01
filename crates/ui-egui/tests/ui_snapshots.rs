//! Offscreen pictures of the whole window, for checking layout and design without a visible
//! screen (locked machines, CI, agents):
//!
//! ```sh
//! TC_UI_SNAPSHOTS=<dir> cargo test -p texturecraft-ui-egui --release --test ui_snapshots -- --ignored
//! ```
//!
//! Writes `<dir>/<name>.png` at 1600x960: the default layout in each brightness, a floating
//! panel and the main windows. Ignored by default: it needs a GPU adapter and takes a few
//! seconds.

use std::path::{Path, PathBuf};

use egui_kittest::Harness;
use texturecraft_ui_egui::TextureCraftApp;
use texturecraft_ui_egui::docking::Panel;
use texturecraft_ui_egui::theme::Brightness;

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
    h.state_mut().ui.dock.focus(Panel::Display);
    save(&mut h, &dir, "panel-display");
    h.state_mut().ui.dock.close(Panel::Display);
    h.state_mut().ui.windows.export = true;
    save(&mut h, &dir, "window-export");
    h.state_mut().ui.windows.export = false;
    h.state_mut().ui.windows.settings = true;
    save(&mut h, &dir, "window-settings");
}
