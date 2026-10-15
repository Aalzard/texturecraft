//! The asset shelf (the Assets dock tab): Painter-style tabs (materials, smart materials, smart
//! masks, alphas, procedurals, environments, textures), all built in and procedural. Search
//! filters across every tab. Materials, smart masks and alphas drag onto the layer stack, a
//! layer row, the viewport or the brush properties ([`ShelfDrag`]).

use egui::{Color32, RichText, Sense, Stroke, Ui, vec2};
use egui_phosphor::regular as ic;
use serde_json::json;
use texturecraft_engine::core::ProceduralKind;
use texturecraft_engine::materials::MATERIALS;
use texturecraft_engine::paint::alphas;
use texturecraft_engine::smart_masks::SMART_MASKS;
use texturecraft_render::ibl::ENVIRONMENTS;

use crate::TextureCraftApp;
use crate::theme::TOKENS;
use crate::widgets::{dnd_release, icon_button};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Materials,
    Smart,
    Masks,
    Alphas,
    Procedurals,
    Environments,
    Textures,
}

const TABS: [(Tab, &str, &str); 7] = [
    (Tab::Materials, ic::SPHERE, "Materials"),
    (Tab::Smart, ic::STACK, "Smart materials"),
    (Tab::Masks, ic::CIRCLE_HALF, "Smart masks"),
    (Tab::Alphas, ic::SHAPES, "Alphas"),
    (Tab::Procedurals, ic::CHECKERBOARD, "Procedurals"),
    (Tab::Environments, ic::SUN_HORIZON, "Environments"),
    (Tab::Textures, ic::IMAGE, "Textures"),
];

/// A built-in asset (by id).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asset {
    Material(&'static str),
    SmartMask(&'static str),
    Alpha(&'static str),
}

/// The drag-and-drop payload of a shelf tile.
#[derive(Clone, Copy, Debug)]
pub struct ShelfDrag {
    pub asset: Asset,
    pub label: &'static str,
}

/// Apply `asset` the way a shelf click does: a material above the selection, a smart mask on
/// the selected layer, an alpha on the brush.
pub fn apply(app: &mut TextureCraftApp, asset: Asset) {
    let (cmd, params) = match asset {
        Asset::Material(id) => ("material.apply", json!({"material": id})),
        Asset::SmartMask(id) => ("mask.smart.apply", json!({"name": id})),
        Asset::Alpha(id) => ("tool.brush", json!({"alpha": {"library": id}})),
    };
    app.run(cmd, params);
}

/// Make `resp` (the viewport, or the brush properties when `materials` is false) a drop target
/// for shelf alphas and materials: outline it while one hovers, apply it on release.
pub fn drop_target(app: &mut TextureCraftApp, ui: &Ui, resp: &egui::Response, materials: bool) {
    let accepts = |d: &ShelfDrag| matches!(d.asset, Asset::Alpha(_)) || (materials && matches!(d.asset, Asset::Material(_)));
    if resp.dnd_hover_payload::<ShelfDrag>().is_some_and(|d| accepts(&d)) {
        ui.painter().rect_stroke(resp.rect.shrink(1.0), 3.0, Stroke::new(2.0, TOKENS.accent), egui::StrokeKind::Inside);
    }
    if let Some(d) = dnd_release::<ShelfDrag>(resp)
        && accepts(&d)
    {
        apply(app, d.asset);
    }
}

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
                Tab::Alphas => alpha_library(app, ui, &ctx, &filter, title),
                Tab::Procedurals => procedurals(app, ui, &ctx, &filter, title),
                Tab::Environments => environments(app, ui, &filter, title),
                Tab::Textures => textures(app, ui, &ctx, &filter, title),
            }
        }
    });
    drag_preview(&ctx);
}

/// The dragged asset's name next to the pointer.
fn drag_preview(ctx: &egui::Context) {
    let (Some(d), Some(pos)) = (egui::DragAndDrop::payload::<ShelfDrag>(ctx), ctx.input(|i| i.pointer.hover_pos())) else { return };
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("shelf_drag")));
    let galley = p.layout_no_wrap(d.label.to_string(), egui::FontId::proportional(11.0), TOKENS.text_strong);
    let rect = egui::Rect::from_min_size(pos + vec2(14.0, 12.0), galley.size() + vec2(12.0, 6.0));
    p.rect_filled(rect, 3.0, TOKENS.row_selected);
    p.rect_stroke(rect, 3.0, Stroke::new(1.0, TOKENS.accent), egui::StrokeKind::Inside);
    p.galley(rect.min + vec2(6.0, 3.0), galley, TOKENS.text_strong);
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

/// The distinct categories of `items`, in order of first appearance.
fn categories<T>(items: &[T], category: impl Fn(&T) -> &'static str) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for c in items.iter().map(category) {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// Built-in materials (or smart materials), grouped by category.
fn materials(app: &mut TextureCraftApp, ui: &mut Ui, ctx: &egui::Context, filter: &str, smart: bool, name: &str) {
    let shown: Vec<_> = MATERIALS.iter().filter(|m| m.smart == smart && matches(filter, &[m.label, m.category, m.id])).collect();
    if shown.is_empty() {
        return;
    }
    title(ui, name);
    for cat in categories(&shown, |m| m.category) {
        header(ui, cat);
        tiles(ui, |ui| {
            for m in shown.iter().filter(|m| m.category == cat) {
                let tex = app.thumbs.material(ctx, m.id);
                let hint = if m.smart {
                    "Add the smart material folder above the selected layer (or drag it into the layer stack)"
                } else {
                    "Add a fill layer with this material above the selected layer (or drag it into the layer stack)"
                };
                let drag = ShelfDrag { asset: Asset::Material(m.id), label: m.label };
                if tile(ui, tex.as_ref(), m.label, hint, Some(drag)).clicked() {
                    apply(app, drag.asset);
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
            let drag = ShelfDrag { asset: Asset::SmartMask(m.id), label: m.label };
            let hint = "Replace the selected layer's mask with this smart mask (or drag it onto a layer)";
            if tile(ui, tex.as_ref(), m.label, hint, Some(drag)).clicked() {
                apply(app, drag.asset);
            }
        }
    });
}

/// The built-in alpha library, grouped by category; the brush's alpha is outlined.
fn alpha_library(app: &mut TextureCraftApp, ui: &mut Ui, ctx: &egui::Context, filter: &str, name: &str) {
    let shown: Vec<&'static alphas::LibraryAlpha> = alphas::library().iter().filter(|a| matches(filter, &[&a.label, a.category, &a.id])).collect();
    if shown.is_empty() {
        return;
    }
    title(ui, name);
    let brush = &app.session.tool.brush;
    let current = if brush.alpha_image.is_none() { brush.alpha_library.clone() } else { None };
    for cat in categories(&shown, |a| a.category) {
        header(ui, cat);
        tiles(ui, |ui| {
            for &a in shown.iter().filter(|a| a.category == cat) {
                let tex = app.thumbs.alpha(ctx, a);
                let drag = ShelfDrag { asset: Asset::Alpha(&a.id), label: &a.label };
                let r = tile(ui, tex.as_ref(), &a.label, "Use as the brush alpha (or drag it onto the viewport or brush properties)", Some(drag));
                if current.as_deref() == Some(a.id.as_str()) {
                    ui.painter().rect_stroke(r.rect, 3.0, Stroke::new(1.0, TOKENS.accent), egui::StrokeKind::Inside);
                }
                if r.clicked() {
                    apply(app, drag.asset);
                }
            }
        });
    }
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
            if tile(ui, tex.as_ref(), k.label(), "Add a fill layer using this pattern", None).clicked() {
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
            let r = tile(ui, Some(&tex), &name, "Add a fill layer using this image", None);
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

/// A shelf tile: thumbnail (a placeholder disc while it renders) and a wrapped label; tiles with
/// a `drag` payload can be dragged.
fn tile(ui: &mut Ui, tex: Option<&egui::TextureHandle>, label: &str, hint: &str, drag: Option<ShelfDrag>) -> egui::Response {
    let sense = if drag.is_some() { Sense::click_and_drag() } else { Sense::click() };
    let (rect, resp) = ui.allocate_exact_size(vec2(72.0, 86.0), sense);
    if let Some(d) = drag {
        resp.dnd_set_drag_payload(d);
    }
    let p = ui.painter();
    if resp.hovered() || resp.dragged() {
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
