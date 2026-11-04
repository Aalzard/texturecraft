use super::controls::{choice, hint, integer, number, section, text_input};
use crate::TextureCraftApp;
use crate::i18n::tr;
use crate::widgets::icon_button;
use egui::Ui;
use egui_phosphor::regular as icon;
use serde_json::json;
use texturecraft_engine::core::{BakeAntialiasing, BakeResolution, BakeSettings, CageType, CommonBakeSettings};

pub(super) fn show(app: &mut TextureCraftApp, ui: &mut Ui, settings: &mut BakeSettings) {
    ui.push_id("common", |ui| {
        let is_enabled = settings.common.is_enabled;
        if section(ui, tr("Common Parameters"), None, |ui| {
            output_size(ui, &mut settings.common);
            choice(
                ui,
                tr("Antialiasing"),
                &mut settings.common.antialiasing,
                &[
                    (BakeAntialiasing::None, tr("None")),
                    (BakeAntialiasing::Subsampling2x2, tr("Subsampling 2x2")),
                    (BakeAntialiasing::Subsampling4x4, tr("Subsampling 4x4")),
                    (BakeAntialiasing::Subsampling8x8, tr("Subsampling 8x8")),
                ],
            );
            integer(ui, tr("Dilation Width"), &mut settings.common.dilation, 0..=128);
            ui.checkbox(&mut settings.common.apply_diffusion, tr("Apply Diffusion"));
        }) {
            settings.common = CommonBakeSettings { is_enabled, ..CommonBakeSettings::default() };
        }
    });
    ui.add_space(8.0);
    ui.push_id("high_poly", |ui| {
        if section(ui, tr("High-Poly Parameters"), None, |ui| {
            ui.checkbox(&mut settings.high_poly.use_low_as_high, tr("Use Low Poly Mesh as High Poly Mesh"));
            high_poly_files(app, ui);
            if !app.session.bake_meshes.high.is_empty() {
                ui.checkbox(&mut settings.high_poly.ignore_backface, tr("Ignore Backface"));
                text_input(ui, tr("Low Poly Mesh Suffix"), &mut settings.high_poly.low_suffix);
                text_input(ui, tr("High Poly Mesh Suffix"), &mut settings.high_poly.high_suffix);
                text_input(ui, tr("Ignore Backface Suffix"), &mut settings.high_poly.ignore_backface_suffix);
            }
        }) {
            settings.high_poly = Default::default();
        }
    });
    if !app.session.bake_meshes.high.is_empty() {
        ui.add_space(8.0);
        ui.push_id("cage", |ui| {
            if section(ui, tr("Cage"), None, |ui| {
                choice(
                    ui,
                    tr("Cage Type"),
                    &mut settings.cage.kind,
                    &[(CageType::DistanceBased, tr("Distance-Based")), (CageType::CustomFile, tr("Custom File"))],
                );
                match settings.cage.kind {
                    CageType::DistanceBased => {
                        ui.add_enabled(false, egui::Button::new(tr("Paint Cage")));
                        number(ui, tr("Max Frontal Distance"), &mut settings.cage.max_frontal, 0.0..=0.05, 4);
                        number(ui, tr("Max Rear Distance"), &mut settings.cage.max_rear, 0.0..=0.05, 4);
                        ui.checkbox(&mut settings.cage.relative_to_bounding_box, tr("Relative to Bounding Box"));
                        ui.checkbox(&mut settings.cage.average_normals, tr("Average Normals"));
                    }
                    CageType::CustomFile => cage_file(app, ui),
                }
            }) {
                settings.cage = Default::default();
            }
        });
        if settings.cage.kind == CageType::DistanceBased {
            ui.add_space(8.0);
            ui.push_id("skew", |ui| {
                if section(ui, tr("Skew Correction"), None, |ui| {
                    skew_brush(app, ui);
                    ui.checkbox(&mut settings.skew.edge_protection, tr("Edge Protection"));
                    number(ui, tr("Edge Distance"), &mut settings.skew.edge_distance, 0.0..=1.0, 2);
                    number(ui, tr("Edge Contrast"), &mut settings.skew.edge_contrast, 0.0..=1.0, 2);
                }) {
                    settings.skew = Default::default();
                }
            });
        }
    }
}

fn output_size(ui: &mut Ui, settings: &mut CommonBakeSettings) {
    ui.label(tr("Output Size"));
    ui.horizontal(|ui| {
        let dropdown_width = ((ui.available_width() - 24.0 - ui.spacing().item_spacing.x * 2.0) / 2.0).max(40.0);
        let resolution = |ui: &mut Ui, id: &str, value: &mut BakeResolution| {
            egui::ComboBox::from_id_salt(id).width(dropdown_width).selected_text(value.pixels().to_string()).show_ui(ui, |ui| {
                for resolution in BakeResolution::ALL {
                    ui.selectable_value(value, resolution, resolution.pixels().to_string());
                }
            });
        };
        resolution(ui, "output_width", &mut settings.resolution);
        let locked = settings.resolution_height.is_none();
        let mut height = settings.resolution_height.unwrap_or(settings.resolution);
        ui.add_enabled_ui(!locked, |ui| resolution(ui, "output_height", &mut height));
        if !locked {
            settings.resolution_height = Some(height);
        }
        if icon_button(ui, if locked { icon::LOCK_SIMPLE } else { icon::LOCK_SIMPLE_OPEN }, tr("Lock Output Size"), locked, 24.0).clicked() {
            settings.resolution_height = if locked { Some(settings.resolution) } else { None };
        }
    });
}

fn high_poly_files(app: &mut TextureCraftApp, ui: &mut Ui) {
    ui.label(tr("High Poly Scene"));
    let meshes: Vec<_> = app.session.bake_meshes.high.iter().map(|mesh| mesh.name.clone()).collect();
    let mut remove = None;
    for (index, name) in meshes.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.add_sized([ui.available_width() - 24.0, 24.0], egui::Label::new(name).truncate()).on_hover_text(name);
            if icon_button(ui, icon::TRASH, tr("Remove"), false, 20.0).clicked() {
                remove = Some(index);
            }
        });
    }
    if meshes.is_empty() {
        hint(ui, tr("No High-Poly Meshes"));
    }
    if let Some(index) = remove {
        app.run("bake.highPoly.remove", json!({ "index": index }));
    }
    if ui.button(tr("Add High-Poly Meshes")).clicked() {
        crate::web_files::import_bake_mesh(app, &ui.ctx().clone(), false);
    }
}

fn cage_file(app: &mut TextureCraftApp, ui: &mut Ui) {
    ui.label(tr("Cage Scene"));
    let cage = app.session.bake_meshes.cage.as_ref().map(|mesh| mesh.name.clone());
    ui.horizontal(|ui| {
        ui.add_sized([ui.available_width() - 52.0, 24.0], egui::Label::new(cage.as_deref().unwrap_or("")).truncate());
        if icon_button(ui, icon::FOLDER_OPEN, tr("Load Cage"), false, 20.0).clicked() {
            crate::web_files::import_bake_mesh(app, &ui.ctx().clone(), true);
        }
        if cage.is_some() && icon_button(ui, icon::TRASH, tr("Remove Cage"), false, 20.0).clicked() {
            app.run("bake.cage.clear", json!({}));
        }
    });
}

fn skew_brush(app: &mut TextureCraftApp, ui: &mut Ui) {
    let mut brush = app.ui.skew_brush;
    if ui.add(egui::Button::new(tr("Paint Skew Correction")).selected(brush.on)).clicked() {
        brush.on = !brush.on;
    }
    if brush.on {
        number(ui, tr("Radius"), &mut brush.radius, 0.002..=0.2, 3);
        number(ui, tr("Value"), &mut brush.value, 0.0..=1.0, 2);
        number(ui, tr("Hardness"), &mut brush.hardness, 0.0..=1.0, 2);
    }
    if brush.on && !app.ui.skew_brush.on && app.ui.layout == crate::state::ViewLayout::Only3D {
        app.ui.layout = crate::state::ViewLayout::Split;
    }
    app.ui.skew_brush = brush;
    if app.session.bake_meshes.skew.is_some() && ui.button(tr("Remove Skew Correction")).clicked() {
        app.run("bake.skew.clear", json!({}));
    }
}
