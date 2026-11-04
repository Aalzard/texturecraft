//! Symmetry in the viewport bar: one mirror button that is lit while any symmetry is active and
//! opens a small popover (mirror X / Y / Z, radial copies and axis, symmetry guides, lazy
//! mouse). The popover stays open while you edit and closes on a click outside it. The Brush
//! properties section shows the same controls, so there is one set of widgets and one state
//! (`session.tool.brush`, `session.tool.show_symmetry_guides`).

use egui::RichText;
use egui_phosphor::regular as ic;
use texturecraft_engine::paint::BrushSettings;

use crate::TextureCraftApp;
use crate::i18n::{tr, trf};
use crate::keymap::{Keymap, shortcut_of, tip};
use crate::theme::Tokens;
use crate::widgets::{icon_button, slider};

/// The command behind the mirror X shortcut.
const MIRROR_COMMAND: &str = "tool.symmetry";
/// The popover's lazy mouse range, in pixels (0 = off).
pub const LAZY_RANGE: std::ops::RangeInclusive<f32> = 0.0..=100.0;

/// The mirror icon for the viewport bar and its popover.
pub fn button(app: &mut TextureCraftApp, ui: &mut egui::Ui) {
    let on = app.session.tool.brush.symmetry.any();
    let hint = tip(&app.ui.keymap, tr("Symmetry and lazy mouse"), MIRROR_COMMAND);
    let resp = icon_button(ui, ic::FLIP_HORIZONTAL, &hint, on, 24.0);
    let popup = ui.make_persistent_id("symmetry_popover");
    if resp.clicked() {
        egui::Popup::toggle_id(ui.ctx(), popup);
    }
    // Open state lives in memory (`toggle_id` above); `from_response` alone shows it always.
    egui::Popup::from_response(&resp).id(popup).open_memory(None).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(230.0);
        let keymap = app.ui.keymap.clone();
        let tool = &mut app.session.tool;
        controls(ui, &mut tool.brush, &mut tool.show_symmetry_guides, &keymap);
    });
}

/// Mirror X / Y / Z, radial copies, the guides checkbox and lazy mouse. Shared by the popover
/// and the Brush properties.
pub fn controls(ui: &mut egui::Ui, brush: &mut BrushSettings, guides: &mut bool, keymap: &Keymap) {
    let tk = Tokens::get(ui.ctx());
    ui.label(RichText::new(tr("Mirror symmetry")).color(tk.text_strong));
    ui.horizontal(|ui| {
        let sym = &mut brush.symmetry;
        for (on, name) in [(&mut sym.x, "X"), (&mut sym.y, "Y"), (&mut sym.z, "Z")] {
            ui.toggle_value(on, name).on_hover_text(trf("Mirror across {axis} through the model center", &[("axis", &name)]));
        }
    });
    let mut copies = f32::from(brush.symmetry.radial);
    if slider(ui, tr("Radial copies"), &mut copies, 0.0..=32.0, 0).changed() {
        brush.symmetry.radial = copies.round().clamp(0.0, 32.0) as u8;
    }
    ui.add_enabled_ui(brush.symmetry.radial > 1, |ui| {
        ui.horizontal(|ui| {
            ui.label(tr("Radial axis"));
            for (k, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                ui.selectable_value(&mut brush.symmetry.axis, k as u8, name);
            }
        });
    });
    ui.checkbox(guides, tr("Show symmetry guides"));
    ui.separator();
    slider(ui, tr("Lazy mouse distance"), &mut brush.lazy, LAZY_RANGE, 0);
    if let Some(key) = shortcut_of(keymap, MIRROR_COMMAND) {
        ui.label(RichText::new(trf("Press {key} to toggle mirror X", &[("key", &key)])).size(10.5).color(tk.text_dim));
    }
}
