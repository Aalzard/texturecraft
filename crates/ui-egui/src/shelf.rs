//! The asset shelf (the Assets dock tab): Painter-style tabs (materials, smart materials, smart
//! masks, procedurals, environments, textures), all built in and procedural. Search filters
//! across every tab.

use egui::{Color32, RichText, Sense, Ui, vec2};
use egui_phosphor::regular as ic;
use serde_json::json;
use texturecraft_engine::core::ProceduralKind;
use texturecraft_engine::materials::MATERIALS;
use texturecraft_engine::smart_masks::SMART_MASKS;
use texturecraft_render::ibl::ENVIRONMENTS;

use crate::TextureCraftApp;
use crate::theme::TOKENS;
use crate::widgets::icon_button;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Materials,
    Smart,
    Masks,
    Procedurals,
    Environments,
    Textures,
}

const TABS: [(Tab, &str, &str); 6] = [
    (Tab::Materials, ic::SPHERE, "Materials"),
    (Tab::Smart, ic::STACK, "Smart materials"),
    (Tab::Masks, ic::CIRCLE_HALF, "Smart masks"),
    (Tab::Procedurals, ic::CHECKERBOARD, "Procedurals"),
    (Tab::Environments, ic::SUN_HORIZON, "Environments"),
    (Tab::Textures, ic::IMAGE, "Textures"),
];

/// The Assets panel body (a dock tab).
pub fn body(app: &mut TextureCraftApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let tab_id = egui::Id::new("shelf_tab");
    let mut tab = ctx.data_mut(|d| *d.get_persisted_mut_or(tab_id, 0usize)).min(TABS.len() - 1);
    let searching = !app.ui.shelf_filter.is_empty();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
        for (i, (_, icon, label)) in TABS.iter().enumerate() {
            if icon_button(ui, icon, label, i == tab && !searching, 24.0).clicked() {
                tab = i;
                app.ui.shelf_filter.clear();
            }
        }
    });
    ctx.data_mut(|d| d.insert_persisted(tab_id, tab));
    ui.horizontal(|ui| {
        ui.label(RichText::new(ic::MAGNIFYING_GLASS).color(TOKENS.text_dim));
        ui.add(egui::TextEdit::singleline(&mut app.ui.shelf_filter).hint_text("Search all assets").desired_width(f32::INFINITY));
    });
    ui.add_space(4.0);
    let filter = app.ui.shelf_filter.to_lowercase();
    egui::ScrollArea::vertical().id_salt("shelf_scroll").auto_shrink([false, false]).show(ui, |ui| {
        // Searching shows matches from every tab, each under its tab's name.
        for (i, (t, _, title)) in TABS.iter().enumerate() {
            if filter.is_empty() && i != tab {
                continue;
            }
            match t {
                Tab::Materials => materials(app, ui, &ctx, &filter, false, title),
                Tab::Smart => {
                    materials(app, ui, &ctx, &filter, true, title);
                    library(app, ui, &filter);
                }
                Tab::Masks => smart_masks(app, ui, &ctx, &filter, title),
                Tab::Procedurals => procedurals(app, ui, &ctx, &filter, title),
                Tab::Environments => environments(app, ui, &filter, title),
                Tab::Textures => textures(app, ui, &ctx, &filter, title),
            }
        }
    });
}

fn matches(filter: &str, texts: &[&str]) -> bool {
    filter.is_empty() || texts.iter().any(|t| t.to_lowercase().contains(filter))
}

fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).strong().color(TOKENS.text_strong));
}

fn header(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(RichText::new(text).size(11.0).color(TOKENS.text_dim));
}

fn tiles(ui: &mut Ui, body: impl FnOnce(&mut Ui)) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        body(ui);
    });
    ui.add_space(6.0);
}

/// Built-in materials (or smart materials), grouped by category.
fn materials(app: &mut TextureCraftApp, ui: &mut Ui, ctx: &egui::Context, filter: &str, smart: bool, name: &str) {
    let shown: Vec<_> = MATERIALS.iter().filter(|m| m.smart == smart && matches(filter, &[m.label, m.category, m.id])).collect();
    if shown.is_empty() {
        return;
    }
    title(ui, name);
    let mut categories: Vec<&str> = Vec::new();
    for m in &shown {
        if !categories.contains(&m.category) {
            categories.push(m.category);
        }
    }
    for cat in categories {
        header(ui, cat);
        tiles(ui, |ui| {
            for m in shown.iter().filter(|m| m.category == cat) {
                let tex = app.thumbs.material(ctx, m.id);
                let hint = if m.smart {
                    "Add the smart material folder above the selected layer"
                } else {
                    "Add a fill layer with this material above the selected layer"
                };
                if tile(ui, tex.as_ref(), m.label, hint).clicked() {
                    app.run("material.apply", json!({"material": m.id}));
                }
            }
        });
    }
}

/// The user's saved smart materials (desktop library folder).
#[cfg(not(target_arch = "wasm32"))]
fn library(app: &mut TextureCraftApp, ui: &mut Ui, filter: &str) {
    if app.library.is_none() {
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(crate::library_dir())
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "tcmat")).collect())
            .unwrap_or_default();
        files.sort();
        app.library = Some(files);
    }
    let files = app.library.clone().unwrap_or_default();
    let name = |f: &std::path::PathBuf| f.file_stem().and_then(|s| s.to_str()).unwrap_or("material").to_string();
    let shown: Vec<_> = files.iter().filter(|f| matches(filter, &[&name(f)])).collect();
    if !filter.is_empty() && shown.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        header(ui, "My smart materials");
        if ui.small_button(ic::ARROW_CLOCKWISE).on_hover_text("Rescan the library folder").clicked() {
            app.library = None;
        }
    });
    if files.is_empty() {
        ui.label(RichText::new("Right-click a layer > Create smart material").size(10.0).color(TOKENS.text_dim));
    }
    for f in shown {
        if ui.selectable_label(false, format!("{}  {}", ic::SPHERE, name(f))).on_hover_text(f.display().to_string()).clicked() {
            app.run("material.load", json!({"path": f.display().to_string()}));
        }
    }
    ui.add_space(6.0);
}

#[cfg(target_arch = "wasm32")]
fn library(_: &mut TextureCraftApp, _: &mut Ui, _: &str) {}

fn smart_masks(app: &mut TextureCraftApp, ui: &mut Ui, ctx: &egui::Context, filter: &str, name: &str) {
    let shown: Vec<_> = SMART_MASKS.iter().filter(|m| matches(filter, &[m.label, m.id])).collect();
    if shown.is_empty() {
        return;
    }
    title(ui, name);
    tiles(ui, |ui| {
        for m in shown {
            let tex = app.thumbs.smart_mask(ctx, m.id);
            if tile(ui, tex.as_ref(), m.label, "Replace the selected layer's mask with this smart mask").clicked() {
                app.run("mask.smart.apply", json!({"name": m.id}));
            }
        }
    });
}

fn procedurals(app: &mut TextureCraftApp, ui: &mut Ui, ctx: &egui::Context, filter: &str, name: &str) {
    let shown: Vec<ProceduralKind> = ProceduralKind::ALL.into_iter().filter(|k| matches(filter, &[k.label()])).collect();
    if shown.is_empty() {
        return;
    }
    title(ui, name);
    tiles(ui, |ui| {
        for k in shown {
            let tex = app.thumbs.procedural(ctx, k);
            if tile(ui, tex.as_ref(), k.label(), "Add a fill layer using this pattern").clicked() {
                app.run("layer.add.fill", json!({"name": k.label()}));
                app.run("layer.fill.set", json!({"channel": "baseColor", "procedural": k.label(), "low": [0.1, 0.1, 0.1], "high": [0.9, 0.9, 0.9]}));
            }
        }
    });
}

fn environments(app: &mut TextureCraftApp, ui: &mut Ui, filter: &str, name: &str) {
    let shown: Vec<&&str> = ENVIRONMENTS.iter().filter(|e| matches(filter, &[e])).collect();
    if shown.is_empty() {
        return;
    }
    title(ui, name);
    for env in shown {
        let on = app.session.doc.project.display.environment == **env;
        if ui.selectable_label(on, format!("{}  {env}", ic::SUN_HORIZON)).clicked() {
            app.run("display.set", json!({"environment": env}));
        }
    }
    ui.add_space(6.0);
}

fn textures(app: &mut TextureCraftApp, ui: &mut Ui, ctx: &egui::Context, filter: &str, name: &str) {
    let names: Vec<String> = app.session.doc.resources.keys().filter(|n| matches(filter, &[n])).cloned().collect();
    if !filter.is_empty() && names.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        title(ui, name);
        if ui.small_button("Import...").clicked() {
            crate::menus::invoke(app, ctx, "resource.import");
        }
    });
    if names.is_empty() {
        ui.label(RichText::new("Import images to use them in fills and masks").size(10.0).color(TOKENS.text_dim));
    }
    tiles(ui, |ui| {
        for name in names {
            let Some(img) = app.session.images.get(&name).cloned() else { continue };
            let tex = app.thumbs.image(ctx, &name, &img);
            let r = tile(ui, Some(&tex), &name, "Add a fill layer using this image");
            if r.clicked() {
                app.run("layer.add.image", json!({"name": name}));
            }
            r.context_menu(|ui| {
                if ui.button("Remove resource").clicked() {
                    app.run("resource.remove", json!({"name": name}));
                    ui.close();
                }
            });
        }
    });
}

/// A shelf tile: thumbnail (a placeholder disc while it renders) and a wrapped label.
fn tile(ui: &mut Ui, tex: Option<&egui::TextureHandle>, label: &str, hint: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(72.0, 86.0), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 3.0, TOKENS.row_hover);
    }
    let img = egui::Rect::from_center_size(rect.center_top() + vec2(0.0, 32.0), vec2(56.0, 56.0));
    match tex {
        Some(t) => {
            p.image(t.id(), img, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            p.circle_filled(img.center(), img.width() * 0.45, TOKENS.row_hover);
        }
    }
    let galley = p.layout(label.to_string(), egui::FontId::proportional(9.5), TOKENS.text, rect.width() - 4.0);
    p.galley(egui::pos2(rect.center().x - galley.size().x * 0.5, img.bottom() + 3.0), galley, TOKENS.text);
    resp.on_hover_text(format!("{label}: {hint}"))
}
