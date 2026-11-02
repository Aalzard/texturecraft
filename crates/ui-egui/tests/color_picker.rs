//! The color picker popup keeps its place while the color changes.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use texturecraft_ui_egui::{theme, widgets};

/// The left edge of the picker's R value.
fn r_left(h: &Harness<'_, [f32; 3]>) -> f32 {
    h.get_all_by_role(Role::SpinButton).next().unwrap().rect().left()
}

#[test]
fn the_popup_stays_put_while_the_color_changes() {
    // A swatch at the right edge of the window, like the Properties panel's.
    // The picker's copy button is an emoji the bundled fonts do not draw.
    let mut h = Harness::builder().with_size(egui::vec2(800.0, 600.0)).allow_missing_glyphs().build_ui_state(
        |ui, color: &mut [f32; 3]| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                widgets::color_swatch(ui, "color", color, 120.0);
            });
        },
        [1.0; 3],
    );
    theme::apply(&h.ctx, theme::Brightness::default(), theme::Accent::default());
    h.run_steps(2);
    let swatch = egui::pos2(800.0 - 60.0, 9.0);
    h.drag_at(swatch);
    h.drop_at(swatch);
    h.run_steps(3);
    let bright = r_left(&h);
    *h.state_mut() = [0.001; 3];
    h.run_steps(3);
    assert_eq!(r_left(&h), bright, "the picker does not move when R, G and B lose digits");
}
