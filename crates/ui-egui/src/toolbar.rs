//! The context tool bar (brush size, flow, stroke opacity, spacing, lazy mouse, symmetry, view
//! toggles), the vertical tool shelf, the right icon rail and the status bar.

use egui::{RichText, vec2};
use egui_phosphor::regular as ic;
use serde_json::json;
use texturecraft_engine::Tool;

use crate::TextureCraftApp;
use crate::docking::Panel;
use crate::i18n::tr;
use crate::keymap::tip;
use crate::state::{Mode, ViewLayout};
use crate::theme::TOKENS;
use crate::widgets::{bar_slider, icon_button};

pub fn tool_icon(t: Tool) -> &'static str {
    match t {
        Tool::Paint => ic::PAINT_BRUSH,
        Tool::Eraser => ic::ERASER,
        Tool::Projection => ic::PROJECTOR_SCREEN,
        Tool::PolygonFill => ic::POLYGON,
        Tool::Smudge => ic::FINGERPRINT,
        Tool::Clone => ic::STAMP,
        Tool::ColorPicker => ic::EYEDROPPER,
        Tool::Path => ic::PATH,
    }
}

pub fn top_bar(app: &mut TextureCraftApp, ui: &mut egui::Ui) {
    egui::Panel::top("tool_bar").exact_size(34.0).frame(egui::Frame::NONE.fill(TOKENS.chrome).inner_margin(egui::Margin::symmetric(8, 2))).show(ui, |ui| {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if app.ui.mode == Mode::Baking {
                ui.label(RichText::new("Bake Mesh Maps").color(TOKENS.text_strong));
                return;
            }
            let tool = app.session.tool.tool;
            ui.label(RichText::new(tool_icon(tool)).size(15.0).color(TOKENS.text_strong)).on_hover_text(tool.label());
            let b = &mut app.session.tool.brush;
            match tool {
                Tool::Paint | Tool::Eraser | Tool::Projection | Tool::Smudge | Tool::Clone | Tool::Path => {
                    let mut size = b.size / 0.25 * 100.0;
                    if bar_slider(ui, "Size", &mut size, 0.05..=100.0, 110.0).changed() {
                        b.size = (size / 100.0 * 0.25).clamp(0.0005, 0.25);
                    }
                    let mut flow = b.flow * 100.0;
                    if bar_slider(ui, "Flow", &mut flow, 0.0..=100.0, 100.0).changed() {
                        b.flow = flow / 100.0;
                    }
                    let mut op = b.opacity * 100.0;
                    if bar_slider(ui, "Stroke opacity", &mut op, 0.0..=100.0, 110.0).changed() {
                        b.opacity = op / 100.0;
                    }
                    let mut sp = b.spacing * 100.0;
                    if bar_slider(ui, "Spacing", &mut sp, 1.0..=200.0, 100.0).changed() {
                        b.spacing = sp / 100.0;
                    }
                    ui.separator();
                    let lazy_on = b.lazy > 0.0;
                    if icon_button(ui, ic::SCRIBBLE, "Lazy mouse", lazy_on, 24.0).clicked() {
                        b.lazy = if lazy_on { 0.0 } else { 20.0 };
                    }
                    if lazy_on {
                        let mut d = b.lazy;
                        if bar_slider(ui, "Distance", &mut d, 1.0..=200.0, 90.0).changed() {
                            b.lazy = d;
                        }
                    }
                    ui.separator();
                    if icon_button(ui, ic::FLIP_HORIZONTAL, "Symmetry (X)", b.symmetry.x, 24.0).clicked() {
                        b.symmetry.x = !b.symmetry.x;
                    }
                    if icon_button(ui, ic::FLIP_VERTICAL, "Symmetry (Y)", b.symmetry.y, 24.0).clicked() {
                        b.symmetry.y = !b.symmetry.y;
                    }
                    if icon_button(ui, ic::CLOUD_RAIN, "Particle brush: particles slide down the surface", b.particles.enabled, 24.0).clicked() {
                        b.particles.enabled = !b.particles.enabled;
                    }
                    if tool == Tool::Smudge {
                        ui.separator();
                        let mut st = app.session.tool.smudge_strength * 100.0;
                        if bar_slider(ui, "Strength", &mut st, 0.0..=100.0, 100.0).changed() {
                            app.session.tool.smudge_strength = (st / 100.0).clamp(0.0, 1.0);
                        }
                    }
                    if tool == Tool::Clone && app.session.tool.clone_source.is_none() {
                        ui.separator();
                        ui.label(RichText::new("Hold V and click the mesh to set the clone source").color(TOKENS.text_dim));
                    }
                    if tool == Tool::Path {
                        ui.separator();
                        path_controls(app, ui);
                    }
                }
                Tool::PolygonFill => {
                    use texturecraft_engine::FillMode;
                    for (m, icon, tip) in [
                        (FillMode::Triangle, ic::POLYGON, "Triangle fill"),
                        (FillMode::UvIsland, ic::SHAPES, "UV island fill"),
                        (FillMode::Mesh, ic::CUBE, "Mesh fill"),
                    ] {
                        if icon_button(ui, icon, tip, app.session.tool.fill_mode == m, 24.0).clicked() {
                            app.session.tool.fill_mode = m;
                        }
                    }
                }
                Tool::ColorPicker => {
                    ui.label(RichText::new("Click the mesh to pick its material into the brush").color(TOKENS.text_dim));
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, ic::CAMERA, "Screenshot of the viewport", false, 24.0).clicked() {
                    app.toast("Use the control channel or `texturecraft-cli render` for screenshots", false);
                }
                ui.separator();
                for (l, icon, label, id) in [
                    (ViewLayout::Only2D, ic::SQUARE, "2D view only", "ui.layout.2d"),
                    (ViewLayout::Only3D, ic::CUBE, "3D view only", "ui.layout.3d"),
                    (ViewLayout::Split, ic::COLUMNS, "3D/2D views", "ui.layout.split"),
                ] {
                    if icon_button(ui, icon, &tip(&app.ui.keymap, label, id), app.ui.layout == l, 24.0).clicked() {
                        app.ui.layout = l;
                    }
                }
                ui.separator();
                let checker = app.session.view == texturecraft_engine::ViewMode::UvChecker;
                if icon_button(ui, ic::CHECKERBOARD, &tip(&app.ui.keymap, "UV checker", "ui.uvChecker"), checker, 24.0).clicked() {
                    let mode = if checker { "material" } else { "uvChecker" };
                    app.run("viewport.view", json!({"mode": mode}));
                }
            });
        });
    });
}

/// Path tool options and actions (top bar and properties).
pub fn path_controls(app: &mut TextureCraftApp, ui: &mut egui::Ui) {
    let path = &mut app.session.tool.path;
    if icon_button(ui, ic::POLYGON, "Closed path", path.closed, 24.0).clicked() {
        path.closed = !path.closed;
    }
    let fill_tip = if path.closed { "Fill the closed path (lasso)" } else { "Fill the path (close it first)" };
    if icon_button(ui, ic::PAINT_BUCKET, fill_tip, path.fill, 24.0).clicked() {
        path.fill = !path.fill;
    }
    let has_points = !path.points.is_empty();
    if ui.add_enabled(has_points, egui::Button::new(format!("{} Apply", ic::CHECK))).on_hover_text("Paint the path with the brush (Enter)").clicked() {
        app.run("tool.path", json!({"commit": true}));
    }
    if ui.add_enabled(has_points, egui::Button::new(format!("{} Cancel", ic::X))).on_hover_text("Remove the path (Esc)").clicked() {
        app.run("tool.path", json!({"clear": true}));
    }
}

pub fn tool_shelf(app: &mut TextureCraftApp, ui: &mut egui::Ui) {
    egui::Panel::left("tool_shelf")
        .resizable(false)
        .exact_size(34.0)
        .frame(egui::Frame::NONE.fill(TOKENS.chrome).inner_margin(egui::Margin::symmetric(3, 6)))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let painting = app.ui.mode == Mode::Painting;
            for t in Tool::ALL {
                let selected = painting && app.session.tool.tool == t;
                let hint = tip(&app.ui.keymap, t.label(), &format!("tool.{}", t.id()));
                if icon_button(ui, tool_icon(t), &hint, selected, 28.0).clicked() {
                    app.ui.mode = Mode::Painting;
                    app.run("tool.select", json!({"tool": t.id()}));
                }
            }
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            let q = &app.session.quick_mask;
            for (id, icon, label, on) in [
                ("viewport.quickMask", ic::SELECTION_BACKGROUND, "Quick mask", q.enabled),
                ("viewport.quickMask.edit", ic::SELECTION, "Edit quick mask", q.editing),
                ("viewport.quickMask.invert", ic::SELECTION_INVERSE, "Invert quick mask", q.inverted),
            ] {
                if icon_button(ui, icon, &tip(&app.ui.keymap, label, id), on, 28.0).clicked() {
                    app.run(id, json!({}));
                }
            }
            ui.add_space(6.0);
            ui.separator();
            ui.add_space(6.0);
            if icon_button(ui, ic::ATOM, &tip(&app.ui.keymap, "Bake mesh maps", "ui.mode.bake"), app.ui.mode == Mode::Baking, 28.0).clicked() {
                app.ui.mode = if app.ui.mode == Mode::Baking { Mode::Painting } else { Mode::Baking };
            }
            if icon_button(ui, ic::APERTURE, &tip(&app.ui.keymap, "Rendering, path traced", "ui.mode.render"), app.ui.mode == Mode::Rendering, 28.0).clicked() {
                app.ui.mode = if app.ui.mode == Mode::Rendering { Mode::Painting } else { Mode::Rendering };
            }
            if icon_button(ui, ic::SPHERE, "Add smart material from the shelf", false, 28.0).clicked() {
                app.ui.dock.focus(Panel::Assets, ui.ctx().content_rect());
            }
        });
}

pub fn rail(app: &mut TextureCraftApp, ui: &mut egui::Ui) {
    egui::Panel::right("rail").resizable(false).exact_size(32.0).frame(egui::Frame::NONE.fill(TOKENS.chrome).inner_margin(egui::Margin::symmetric(3, 6))).show(
        ui,
        |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            if icon_button(ui, ic::EXPORT, &tip(&app.ui.keymap, "Export textures", "ui.export"), app.ui.windows.export, 26.0).clicked() {
                app.ui.windows.export = !app.ui.windows.export;
            }
            let screen = ui.ctx().content_rect();
            for (panel, icon, id) in [
                (Panel::TextureSetSettings, ic::STACK, "ui.view.textureSetSettings"),
                (Panel::Display, ic::MONITOR, "ui.view.display"),
                (Panel::Shader, ic::SLIDERS_HORIZONTAL, "ui.view.shader"),
                (Panel::History, ic::CLOCK_COUNTER_CLOCKWISE, "ui.view.history"),
            ] {
                if icon_button(ui, icon, &tip(&app.ui.keymap, tr(panel.name()), id), app.ui.dock.is_visible(panel), 26.0).clicked() {
                    app.ui.dock.toggle(panel, screen);
                }
            }
        },
    );
}

pub fn status_bar(app: &mut TextureCraftApp, ui: &mut egui::Ui) {
    egui::Panel::bottom("status_bar").exact_size(22.0).frame(egui::Frame::NONE.fill(TOKENS.chrome).inner_margin(egui::Margin::symmetric(10, 0))).show(
        ui,
        |ui| {
            ui.horizontal_centered(|ui| {
                if let Some(p) = app.bake_progress() {
                    ui.add(egui::ProgressBar::new(p).desired_width(160.0).desired_height(10.0).text(RichText::new("Baking").size(10.0)));
                    if ui.small_button("Cancel").clicked() {
                        app.cancel_bake();
                    }
                } else if let Some(p) = app.gpu.as_ref().and_then(|g| g.composite_progress(&app.session)) {
                    ui.add(egui::ProgressBar::new(p).desired_width(160.0).desired_height(10.0).text(RichText::new("Compositing").size(10.0)));
                    ui.ctx().request_repaint();
                } else if let Some(msg) = app.session.log.last() {
                    ui.label(RichText::new(msg).size(11.0).color(TOKENS.text_dim));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let gpu = app.gpu.as_ref().map(|g| g.adapter.as_str()).unwrap_or("no GPU");
                    ui.label(RichText::new(format!("Version: {} ({gpu})", texturecraft_engine::VERSION)).size(11.0).color(TOKENS.text_dim));
                    ui.separator();
                    let m = app.session.memory();
                    let mb = |b: usize| b as f64 / (1024.0 * 1024.0);
                    let total = mb(m.total());
                    let decimals = if total < 10.0 { 1 } else { 0 };
                    ui.label(RichText::new(format!("Layer memory: {total:.decimals$} MB")).size(11.0).color(TOKENS.text_dim)).on_hover_text(format!(
                        "Painted layer pixels {:.1} MB, composited channels {:.1} MB, compositing cache {:.1} MB",
                        mb(m.pixels),
                        mb(m.composites),
                        mb(m.cache)
                    ));
                    ui.separator();
                    ui.label(RichText::new(format!("UI {:.1} ms", app.ui_ms)).size(11.0).color(TOKENS.text_dim));
                    ui.add_space(vec2(0.0, 0.0).x);
                });
            });
        },
    );
}
