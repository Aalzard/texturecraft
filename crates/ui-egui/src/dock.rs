//! Panel bodies of the right column: Texture Set List, Layers, Texture Set Settings, Properties
//! (hosted as dock tabs by `docking`).

use egui::{Color32, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use egui_phosphor::regular as ic;
use serde_json::{Value, json};
use texturecraft_engine::core::{BakedMap, BlendMode, Channel, EffectKind, Fill, Filter, GeneratorKind, Layer, LayerId, LayerKind, ProceduralKind, Source};
use texturecraft_engine::paint::{BrushAlpha, alphas};
use texturecraft_engine::{StackTarget, Tool};

use crate::TextureCraftApp;
use crate::shelf::{Asset, ShelfDrag};
use crate::state::PropsTab;
use crate::theme::{TOKENS, caption};
use crate::widgets::{chip, color_swatch, dnd_release, elided, icon_button, section, slider};

const TAG_COLORS: [Color32; 8] = [
    Color32::from_rgb(0xc0, 0x40, 0x40),
    Color32::from_rgb(0xd0, 0x80, 0x30),
    Color32::from_rgb(0xc8, 0xb0, 0x30),
    Color32::from_rgb(0x58, 0xa0, 0x40),
    Color32::from_rgb(0x30, 0xa0, 0x90),
    Color32::from_rgb(0x40, 0x78, 0xd0),
    Color32::from_rgb(0x80, 0x58, 0xc8),
    Color32::from_rgb(0xb0, 0x50, 0xa0),
];

pub(crate) fn texture_set_list(app: &mut TextureCraftApp, ui: &mut Ui) {
    let sets: Vec<(String, u32, bool)> = app.session.doc.project.texture_sets.iter().map(|t| (t.name.clone(), t.resolution, t.visible)).collect();
    let active = app.session.ts();
    egui::ScrollArea::vertical().id_salt("ts_scroll").show(ui, |ui| {
        for (i, (name, res, visible)) in sets.into_iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
            let p = ui.painter();
            let bg = if i == active {
                TOKENS.row_selected
            } else if resp.hovered() {
                TOKENS.row_hover
            } else {
                TOKENS.panel
            };
            p.rect_filled(rect, 2.0, bg);
            if i == active {
                p.rect_stroke(rect, 2.0, Stroke::new(1.0, TOKENS.accent), egui::StrokeKind::Inside);
            }
            let eye = Rect::from_min_size(rect.min + vec2(4.0, 3.0), vec2(16.0, 16.0));
            let eye_resp = ui.interact(eye, ui.id().with(("ts_eye", i)), Sense::click()).on_hover_text("Show or hide the texture set");
            p.text(eye.center(), egui::Align2::CENTER_CENTER, if visible { ic::EYE } else { ic::EYE_SLASH }, egui::FontId::proportional(13.0), TOKENS.text);
            // Narrow panels drop the shader label, then the resolution, before cutting the name.
            let cy = rect.center().y;
            let mut right = rect.right() - 6.0;
            if rect.width() >= 260.0 {
                let g = p.layout_no_wrap("Main shader".into(), egui::FontId::proportional(11.0), TOKENS.text_dim);
                right -= g.size().x;
                p.galley(pos2(right, cy - g.size().y * 0.5), g, TOKENS.text_dim);
                right -= 10.0;
            }
            if rect.width() >= 170.0 {
                let g = p.layout_no_wrap(format!("{res}x{res}"), egui::FontId::proportional(11.0), TOKENS.text_dim);
                let x = (rect.left() + rect.width() * 0.48).min(right - g.size().x);
                p.galley(pos2(x, cy - g.size().y * 0.5), g, TOKENS.text_dim);
                right = x - 8.0;
            }
            let color = if i == active { TOKENS.text_strong } else { TOKENS.text };
            let g = elided(p, name, 12.0, color, right - rect.left() - 26.0);
            p.galley(pos2(rect.left() + 26.0, cy - g.size().y * 0.5), g, color);
            if eye_resp.clicked() {
                app.run("textureSet.visible", json!({"ts": i}));
            } else if resp.clicked() {
                app.run("textureSet.select", json!({"ts": i}));
            }
        }
    });
}

// ---------------------------------------------------------------------------------------------
// Layers

pub(crate) fn layers(app: &mut TextureCraftApp, ui: &mut Ui) {
    // Wraps in a narrow column instead of widening the rows below past the panel.
    ui.horizontal_wrapped(|ui| {
        let chans = app.session.doc.project.texture_set(app.session.ts()).map(|t| t.channel_list()).unwrap_or_default();
        egui::ComboBox::from_id_salt("layer_channel").width(86.0).selected_text(app.ui.layer_channel.label()).show_ui(ui, |ui| {
            for c in chans {
                ui.selectable_value(&mut app.ui.layer_channel, c, c.label());
            }
        });
        let has = app.session.selected_layer().is_some();
        let selected = app.session.selected_layer().and_then(|id| app.session.doc.project.layers.get(&id)).cloned();
        ui.menu_button(RichText::new(ic::MAGIC_WAND).size(14.0), |ui| match &selected {
            Some(l) => effect_menu(app, ui, l),
            None => {
                ui.label(RichText::new("Select a layer").color(TOKENS.text_dim));
            }
        })
        .response
        .on_hover_text("Add effect");
        ui.menu_button(RichText::new(ic::SELECTION_BACKGROUND).size(14.0), |ui| {
            for (label, id) in
                [("Add white mask", "mask.add.white"), ("Add black mask", "mask.add.black"), ("Add mask with color selection", "mask.add.colorSelection")]
            {
                if ui.add_enabled(has, egui::Button::new(label)).clicked() {
                    app.run(id, Value::Null);
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text("Add mask");
        if icon_button(ui, ic::PAINT_BRUSH, "Add paint layer", false, 22.0).clicked() {
            app.run("layer.add.paint", Value::Null);
        }
        if icon_button(ui, ic::PAINT_BUCKET, "Add fill layer", false, 22.0).clicked() {
            app.run("layer.add.fill", Value::Null);
        }
        if icon_button(ui, ic::FOLDER_SIMPLE, "Add folder", false, 22.0).clicked() {
            app.run("layer.add.group", Value::Null);
        }
        if icon_button(ui, ic::TRASH, &crate::keymap::tip(&app.ui.keymap, "Remove layer", "layer.remove"), false, 22.0).clicked() {
            app.run("layer.remove", Value::Null);
        }
    });
    ui.add_space(2.0);
    let ts = app.session.ts();
    let stack: Vec<LayerId> = app.session.doc.project.texture_sets.get(ts).map(|t| t.stack.iter().rev().copied().collect()).unwrap_or_default();
    egui::ScrollArea::vertical().id_salt("layers_scroll").auto_shrink([false, false]).show(ui, |ui| {
        for id in stack {
            layer_tree(app, ui, id, 0);
        }
        // Click on empty space deselects nothing; the space below the rows drops at the bottom.
        let free = ui.available_height();
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), if free.is_finite() { free.max(24.0) } else { 24.0 }), Sense::hover());
        if let Some(dragged) = dnd_release::<u64>(&resp) {
            let bottom = app.session.doc.project.texture_sets.get(ts).and_then(|t| t.stack.first().copied());
            if bottom.map(|b| b.0) != Some(*dragged) {
                app.run("layer.move", json!({"layer": *dragged, "index": 0}));
            }
        }
        if let Some(d) = dnd_release::<ShelfDrag>(&resp)
            && let Asset::Material(m) = d.asset
        {
            app.run("material.apply", json!({"material": m, "index": 0}));
        }
        let material = resp.dnd_hover_payload::<ShelfDrag>().is_some_and(|d| matches!(d.asset, Asset::Material(_)));
        if resp.dnd_hover_payload::<u64>().is_some() || material {
            ui.painter().line_segment([rect.left_top(), rect.right_top()], Stroke::new(2.0, TOKENS.accent));
        }
    });
}

/// Whether the selection targets the layer content (the content or one of its effects) rather
/// than the mask.
fn targets_content(app: &TextureCraftApp) -> bool {
    let p = &app.session.doc.project;
    match app.session.target {
        StackTarget::Content => true,
        StackTarget::Mask => false,
        StackTarget::Effect(e) => p.effect(e).and_then(|(owner, _)| p.layers.get(&owner)).is_some_and(|l| l.content_effects.iter().any(|x| x.id == e)),
    }
}

/// "Add effect" items for `layer` (toolbar and context menu). Effects that fit both stacks go
/// to the content when it is the selected target; generators and compare always go to the
/// mask, anchor points to the content.
fn effect_menu(app: &mut TextureCraftApp, ui: &mut Ui, layer: &Layer) {
    let content = !layer.is_group() && app.session.selected_layer() == Some(layer.id) && targets_content(app);
    let t = if content { "content" } else { "mask" };
    ui.label(caption(if content { "Add to the layer content" } else { "Add to the mask" }));
    let items = [
        ("Add generator", "effect.add.generator", json!({})),
        ("Add paint", "effect.add.paint", json!({"target": t})),
        ("Add fill", "effect.add.fill", json!({"target": t})),
        ("Add levels", "effect.add.levels", json!({"target": t})),
        ("Add filter: blur", "effect.add.filter", json!({"target": t, "filter": "blur", "radius": 8})),
        ("Add filter: sharpen", "effect.add.filter", json!({"target": t, "filter": "sharpen"})),
        ("Add filter: invert", "effect.add.filter", json!({"target": t, "filter": "invert"})),
        ("Add filter: threshold", "effect.add.filter", json!({"target": t, "filter": "threshold"})),
        ("Add filter: HSL", "effect.add.filter", json!({"target": t, "filter": "hsl"})),
        ("Add compare mask", "effect.add.compare", json!({})),
        ("Add anchor point", "effect.add.anchor", json!({})),
    ];
    for (label, id, p) in items {
        let enabled = id != "effect.add.anchor" || !layer.is_group();
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            if app.session.selected_layer() != Some(layer.id) {
                app.run("layer.select", json!({"layer": layer.id.0}));
            }
            app.run(id, with(&p, json!({"layer": layer.id.0})));
            ui.close();
        }
    }
}

fn layer_tree(app: &mut TextureCraftApp, ui: &mut Ui, id: LayerId, depth: usize) {
    let Some(layer) = app.session.doc.project.layers.get(&id).cloned() else { return };
    layer_row(app, ui, &layer, depth);
    for e in layer.content_effects.iter().rev() {
        effect_row(app, ui, &layer, e.id, &e.name, e.visible, depth + 1, ic::SPHERE);
    }
    if let Some(m) = &layer.mask {
        for e in m.effects.iter().rev() {
            effect_row(app, ui, &layer, e.id, &e.name, e.visible, depth + 1, ic::MAGIC_WAND);
        }
    }
    if let LayerKind::Group { children, collapsed } = &layer.kind
        && !collapsed
        && depth < 12
    {
        for c in children.iter().rev() {
            layer_tree(app, ui, *c, depth + 1);
        }
    }
}

fn layer_row(app: &mut TextureCraftApp, ui: &mut Ui, layer: &Layer, depth: usize) {
    let selected = app.session.selected_layer() == Some(layer.id);
    let h = 38.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click_and_drag());
    let p = ui.painter().clone();
    let bg = if selected && app.session.target == StackTarget::Content {
        TOKENS.row_selected
    } else if resp.hovered() {
        TOKENS.row_hover
    } else {
        TOKENS.row
    };
    p.rect_filled(rect.shrink2(vec2(0.0, 1.0)), 2.0, bg);
    if selected {
        p.rect_stroke(rect.shrink2(vec2(0.0, 1.0)), 2.0, Stroke::new(1.0, TOKENS.accent), egui::StrokeKind::Inside);
    }
    if let Some(tag) = layer.color_tag.and_then(|t| TAG_COLORS.get(t as usize)) {
        p.rect_filled(Rect::from_min_size(rect.min + vec2(0.0, 1.0), vec2(3.0, h - 2.0)), 0.0, *tag);
    }
    let x0 = rect.left() + 6.0 + depth as f32 * 14.0;
    // Eye.
    let eye = Rect::from_center_size(pos2(x0 + 8.0, rect.center().y), vec2(18.0, 18.0));
    let eye_resp = ui.interact(eye, ui.id().with(("eye", layer.id.0)), Sense::click()).on_hover_text("Show or hide the layer");
    p.text(
        eye.center(),
        egui::Align2::CENTER_CENTER,
        if layer.visible { ic::EYE } else { ic::EYE_SLASH },
        egui::FontId::proportional(13.0),
        if layer.visible { TOKENS.text } else { TOKENS.text_dim },
    );
    if eye_resp.clicked() {
        app.run("layer.visible", json!({"layer": layer.id.0}));
    }
    // Group caret.
    let mut x = x0 + 20.0;
    if let LayerKind::Group { collapsed, .. } = layer.kind {
        let caret = Rect::from_center_size(pos2(x + 6.0, rect.center().y), vec2(14.0, 14.0));
        let cr = ui.interact(caret, ui.id().with(("caret", layer.id.0)), Sense::click()).on_hover_text("Expand or collapse the folder");
        p.text(
            caret.center(),
            egui::Align2::CENTER_CENTER,
            if collapsed { ic::CARET_RIGHT } else { ic::CARET_DOWN },
            egui::FontId::proportional(11.0),
            TOKENS.text_dim,
        );
        if cr.clicked()
            && let Some(LayerKind::Group { collapsed, .. }) = app.session.doc.project.layers.get_mut(&layer.id).map(|l| &mut l.kind)
        {
            *collapsed = !*collapsed;
        }
        x += 14.0;
    }
    // Thumbnail.
    let thumb = Rect::from_min_size(pos2(x, rect.top() + 5.0), vec2(28.0, 28.0));
    p.rect_filled(thumb, 2.0, Color32::from_rgb(0x1a, 0x1a, 0x1a));
    match &layer.kind {
        LayerKind::Fill(f) => {
            let c = match f.source(Channel::BaseColor) {
                Source::Uniform(v) => v,
                Source::Procedural { low, high, .. } => [(low[0] + high[0]) * 0.5, (low[1] + high[1]) * 0.5, (low[2] + high[2]) * 0.5],
                Source::Image { .. } | Source::Anchor { .. } => [0.5; 3],
            };
            crate::thumbs::paint_sphere(&p, thumb.center(), 12.0, c);
        }
        LayerKind::Paint => {
            p.text(thumb.center(), egui::Align2::CENTER_CENTER, ic::PAINT_BRUSH, egui::FontId::proportional(15.0), TOKENS.text);
        }
        LayerKind::Group { .. } => {
            p.text(thumb.center(), egui::Align2::CENTER_CENTER, ic::FOLDER_SIMPLE, egui::FontId::proportional(16.0), TOKENS.text);
        }
        LayerKind::Instance { .. } => {
            p.text(thumb.center(), egui::Align2::CENTER_CENTER, ic::LINK, egui::FontId::proportional(16.0), TOKENS.accent);
        }
    }
    x = thumb.right() + 8.0;
    // An instance draws its source's content, blending and (unless it has its own) mask.
    let project = &app.session.doc.project;
    let source = layer.instance_source().map(|src| {
        let set = project.locate(src).and_then(|(ts, _, _)| project.texture_sets.get(ts)).map(|t| t.name.as_str()).unwrap_or("?");
        project.layers.get(&src).map_or_else(|| "missing source".to_string(), |l| format!("{} · {set}", l.name))
    });
    let mask = project.layers.get(&project.mask_owner(layer.id)).and_then(|l| l.mask.clone());
    let blend = project.content_layer(layer.id).and_then(|l| l.blend(app.ui.layer_channel));
    // Mask thumbnail.
    let right_w = 46.0;
    let mask_rect = Rect::from_min_size(pos2(rect.right() - right_w - 30.0, rect.top() + 7.0), vec2(24.0, 24.0));
    if let Some(m) = &mask {
        let mask_sel = selected && !targets_content(app);
        p.rect_filled(
            mask_rect,
            1.0,
            if m.background == texturecraft_engine::core::MaskBackground::White { Color32::from_gray(225) } else { Color32::from_gray(18) },
        );
        p.rect_stroke(
            mask_rect,
            1.0,
            Stroke::new(if mask_sel { 1.5 } else { 1.0 }, if mask_sel { TOKENS.accent } else { Color32::from_gray(80) }),
            egui::StrokeKind::Outside,
        );
        if !m.effects.is_empty() {
            p.text(mask_rect.center(), egui::Align2::CENTER_CENTER, ic::MAGIC_WAND, egui::FontId::proportional(11.0), Color32::from_rgb(0x80, 0x80, 0x80));
        }
        if !m.enabled {
            p.line_segment([mask_rect.left_top(), mask_rect.right_bottom()], Stroke::new(1.5, TOKENS.danger));
        }
        let mr =
            ui.interact(mask_rect, ui.id().with(("mask", layer.id.0)), Sense::click()).on_hover_text("Select the mask (Alt+click: show it in the viewport)");
        if mr.clicked() {
            app.run("layer.select", json!({"layer": layer.id.0, "target": "mask"}));
            if ui.input(|i| i.modifiers.alt) {
                let showing = app.session.view == texturecraft_engine::ViewMode::Mask(layer.id);
                let p = if showing { json!({"mode": "material"}) } else { json!({"mode": "mask", "layer": layer.id.0}) };
                app.run("viewport.view", p);
            }
        }
        if app.session.view == texturecraft_engine::ViewMode::Mask(layer.id) {
            p.rect_stroke(mask_rect.expand(2.0), 1.0, Stroke::new(1.5, Color32::from_rgb(0xe0, 0xb0, 0x40)), egui::StrokeKind::Outside);
        }
    } else {
        p.rect_stroke(mask_rect, 1.0, Stroke::new(1.0, Color32::from_gray(52)), egui::StrokeKind::Inside);
    }
    // Name (or rename editor).
    let name_rect = Rect::from_min_max(pos2(x, rect.top() + 4.0), pos2(mask_rect.left() - 4.0, rect.bottom() - 4.0));
    let renaming = app.ui.rename.as_ref().is_some_and(|(r, _)| *r == layer.id.0);
    if renaming {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(name_rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
        if let Some((_, buf)) = app.ui.rename.as_mut() {
            let r = child.add(egui::TextEdit::singleline(buf).desired_width(name_rect.width()));
            r.request_focus();
            if r.lost_focus() {
                let name = buf.clone();
                app.ui.rename = None;
                app.run("layer.rename", json!({"layer": layer.id.0, "name": name}));
            }
        }
    } else {
        let galley = elided(&p, layer.name.clone(), 12.0, if layer.visible { TOKENS.text_strong } else { TOKENS.text_dim }, name_rect.width());
        let y = if source.is_some() { rect.center().y - 7.0 } else { rect.center().y };
        p.galley(pos2(name_rect.left(), y - galley.size().y * 0.5), galley, TOKENS.text);
        if let Some(src) = source {
            let src = elided(&p, format!("{} {src}", ic::LINK_SIMPLE), 10.0, TOKENS.text_dim, name_rect.width());
            p.galley(pos2(name_rect.left(), rect.center().y + 7.0 - src.size().y * 0.5), src, TOKENS.text_dim);
        }
    }
    // Blend mode and opacity of the displayed channel.
    let ch = app.ui.layer_channel;
    let blend_rect = Rect::from_min_size(pos2(rect.right() - right_w - 2.0, rect.top() + 3.0), vec2(right_w, 16.0));
    let op_rect = Rect::from_min_size(pos2(rect.right() - right_w - 2.0, rect.top() + 19.0), vec2(right_w, 16.0));
    if let Some(b) = blend {
        let br = ui.interact(blend_rect, ui.id().with(("blend", layer.id.0)), Sense::click()).on_hover_text("Blending mode of the displayed channel");
        p.text(blend_rect.right_center() - vec2(10.0, 0.0), egui::Align2::RIGHT_CENTER, b.mode.short(), egui::FontId::proportional(11.0), TOKENS.text);
        p.text(blend_rect.right_center(), egui::Align2::RIGHT_CENTER, ic::CARET_DOWN, egui::FontId::proportional(8.0), TOKENS.text_dim);
        egui::Popup::menu(&br).show(|ui| {
            ui.set_min_width(190.0);
            for m in BlendMode::MENU {
                match m {
                    Some(m) => {
                        if ui.selectable_label(m == b.mode, m.label()).clicked() {
                            app.run("layer.blend", json!({"layer": layer.id.0, "channel": ch.id(), "mode": format!("{m:?}")}));
                            ui.close();
                        }
                    }
                    None => {
                        ui.separator();
                    }
                }
            }
        });
        let mut op = (b.opacity * 100.0).round();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(op_rect).layout(egui::Layout::right_to_left(egui::Align::Center)));
        let r = child.add(egui::DragValue::new(&mut op).range(0.0..=100.0).speed(0.5).max_decimals(0));
        if r.changed() {
            app.run("layer.blend", json!({"layer": layer.id.0, "channel": ch.id(), "opacity": op / 100.0}));
        }
    } else {
        p.text(blend_rect.right_center(), egui::Align2::RIGHT_CENTER, "off", egui::FontId::proportional(11.0), TOKENS.text_dim);
    }
    // Interaction: select, rename, drag & drop, context menu.
    if resp.clicked() && !eye_resp.clicked() {
        app.run("layer.select", json!({"layer": layer.id.0}));
    }
    if resp.double_clicked() {
        app.ui.rename = Some((layer.id.0, layer.name.clone()));
    }
    if resp.drag_started() {
        resp.dnd_set_drag_payload(layer.id.0);
    }
    // Drops: a layer or a shelf material lands above this row (inside a folder when dropped on
    // its middle); a shelf smart mask replaces this layer's mask.
    let inside = layer.is_group() && ui.input(|i| i.pointer.hover_pos()).is_some_and(|p| (p.y - rect.center().y).abs() < h * 0.25);
    let shelf = resp.dnd_hover_payload::<ShelfDrag>().map(|d| d.asset);
    let moving = resp.dnd_hover_payload::<u64>().is_some_and(|src| *src != layer.id.0);
    if moving || matches!(shelf, Some(Asset::Material(_))) {
        if inside {
            p.rect_stroke(rect, 2.0, Stroke::new(2.0, TOKENS.accent), egui::StrokeKind::Inside);
        } else {
            p.line_segment([rect.left_top(), rect.right_top()], Stroke::new(2.0, TOKENS.accent));
        }
    } else if matches!(shelf, Some(Asset::SmartMask(_))) {
        p.rect_stroke(mask_rect.expand(2.0), 2.0, Stroke::new(2.0, TOKENS.accent), egui::StrokeKind::Outside);
    }
    if let Some(src) = dnd_release::<u64>(&resp)
        && *src != layer.id.0
    {
        app.run("layer.move", json!({"layer": *src, "above": layer.id.0, "inside": inside}));
    }
    match dnd_release::<ShelfDrag>(&resp).map(|d| d.asset) {
        Some(Asset::Material(m)) => {
            app.run("material.apply", json!({"material": m, "above": layer.id.0, "inside": inside}));
        }
        Some(Asset::SmartMask(m)) => {
            app.run("mask.smart.apply", json!({"name": m, "layer": layer.id.0}));
        }
        Some(Asset::Alpha(_)) | None => {}
    }
    resp.context_menu(|ui| layer_context_menu(app, ui, layer));
}

/// One effect of a layer's content (`glyph` sphere) or mask (`glyph` wand) stack.
#[allow(clippy::too_many_arguments)]
fn effect_row(app: &mut TextureCraftApp, ui: &mut Ui, layer: &Layer, eid: LayerId, name: &str, visible: bool, depth: usize, glyph: &str) {
    let selected = app.session.selected_layer() == Some(layer.id) && app.session.target == StackTarget::Effect(eid);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
    let p = ui.painter().clone();
    p.rect_filled(
        rect.shrink2(vec2(0.0, 1.0)),
        2.0,
        if selected {
            TOKENS.row_selected
        } else if resp.hovered() {
            TOKENS.row_hover
        } else {
            TOKENS.panel
        },
    );
    if selected {
        p.rect_stroke(rect.shrink2(vec2(0.0, 1.0)), 2.0, Stroke::new(1.0, TOKENS.accent), egui::StrokeKind::Inside);
    }
    let x0 = rect.left() + 6.0 + depth as f32 * 14.0;
    let eye = Rect::from_center_size(pos2(x0 + 8.0, rect.center().y), vec2(16.0, 16.0));
    let er = ui.interact(eye, ui.id().with(("eff_eye", eid.0)), Sense::click()).on_hover_text("Enable or disable the effect");
    p.text(eye.center(), egui::Align2::CENTER_CENTER, if visible { ic::EYE } else { ic::EYE_SLASH }, egui::FontId::proportional(12.0), TOKENS.text_dim);
    p.text(pos2(x0 + 22.0, rect.center().y), egui::Align2::LEFT_CENTER, glyph, egui::FontId::proportional(11.0), TOKENS.text_dim);
    p.text(pos2(x0 + 38.0, rect.center().y), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(11.5), TOKENS.text);
    if er.clicked() {
        app.run("effect.set", json!({"effect": eid.0, "visible": !visible}));
    } else if resp.clicked() {
        app.run("layer.select", json!({"layer": layer.id.0, "target": eid.0}));
    }
    resp.context_menu(|ui| {
        if ui.button("Remove effect").clicked() {
            app.run("effect.remove", json!({"effect": eid.0}));
            ui.close();
        }
    });
}

fn layer_context_menu(app: &mut TextureCraftApp, ui: &mut Ui, layer: &Layer) {
    ui.set_min_width(230.0);
    let lid = layer.id.0;
    let run = |app: &mut TextureCraftApp, ui: &mut Ui, label: &str, id: &str, p: Value, enabled: bool| {
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            app.run("layer.select", json!({"layer": lid}));
            app.run(id, p);
            ui.close();
        }
    };
    // An instance without a mask of its own edits its source's; adding one overrides it.
    let project = &app.session.doc.project;
    let has_mask = project.layers.get(&project.mask_owner(layer.id)).is_some_and(|l| l.mask.is_some());
    run(app, ui, "Add white mask", "mask.add.white", json!({"layer": lid}), layer.mask.is_none());
    run(app, ui, "Add black mask", "mask.add.black", json!({"layer": lid}), layer.mask.is_none());
    run(app, ui, "Add mask with color selection", "mask.add.colorSelection", json!({"layer": lid}), true);
    ui.separator();
    run(app, ui, "Toggle mask", "mask.toggle", json!({"layer": lid}), has_mask);
    run(app, ui, "Invert mask background", "mask.invertBackground", json!({"layer": lid}), has_mask);
    run(app, ui, "Invert mask", "mask.invert", json!({"layer": lid}), has_mask);
    run(app, ui, "Clear mask", "mask.clear", json!({"layer": lid}), has_mask);
    run(app, ui, "Remove mask", "mask.remove", json!({"layer": lid}), has_mask);
    ui.separator();
    run(app, ui, "Flatten layer               Ctrl+M", "layer.flatten", json!({"layer": lid}), !layer.is_group());
    #[cfg(not(target_arch = "wasm32"))]
    if ui.button("Create smart material").clicked() {
        let file = crate::library_dir().join(format!("{}.tcmat", texturecraft_engine::formats::sanitize_resource_name(&layer.name)));
        if let Some(r) = app.run("material.save", json!({"layer": lid, "path": file.display().to_string(), "name": layer.name})) {
            let p = r.get("path").and_then(|p| p.as_str()).unwrap_or_default().to_string();
            app.toast(format!("Smart material saved to {p}"), false);
            app.library = None;
        }
        ui.close();
    }
    ui.separator();
    run(app, ui, "Cut layer(s)                Ctrl+X", "layer.cut", json!({"layer": lid}), true);
    run(app, ui, "Copy layer(s)               Ctrl+C", "layer.copy", json!({"layer": lid}), true);
    run(app, ui, "Paste layer(s)              Ctrl+V", "layer.paste", json!({}), app.session.clipboard.is_some());
    let paste_instance = crate::menus::enabled(app, "layer.pasteInstance");
    run(app, ui, "Paste as instance", "layer.pasteInstance", json!({}), paste_instance);
    run(app, ui, "Duplicate layer(s)          Ctrl+D", "layer.duplicate", json!({"layer": lid}), true);
    run(app, ui, "Instantiate across texture sets", "layer.instantiateAll", json!({"layer": lid}), app.session.set_count() > 1);
    run(app, ui, "Copy to all texture sets", "layer.instantiateAll", json!({"layer": lid, "copy": true}), app.session.set_count() > 1);
    run(app, ui, "Remove layer(s)             Del", "layer.remove", json!({"layer": lid}), true);
    run(app, ui, "Group layer(s)              Ctrl+G", "layer.group", json!({"layer": lid}), true);
    ui.separator();
    ui.horizontal(|ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
        ui.painter().rect_stroke(r, 2.0, Stroke::new(1.0, TOKENS.text_dim), egui::StrokeKind::Inside);
        ui.painter().line_segment([r.left_bottom(), r.right_top()], Stroke::new(1.0, TOKENS.danger));
        if resp.clicked() {
            app.run("layer.colorTag", json!({"layer": lid}));
        }
        for (i, c) in TAG_COLORS.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
            ui.painter().rect_filled(r, 2.0, *c);
            if resp.clicked() {
                app.run("layer.colorTag", json!({"layer": lid, "tag": i}));
            }
        }
    });
    ui.separator();
    effect_menu(app, ui, layer);
}

// ---------------------------------------------------------------------------------------------
// Texture set settings

pub(crate) fn texture_set_settings(app: &mut TextureCraftApp, ui: &mut Ui) {
    let ts = app.session.ts();
    let Some(set) = app.session.doc.project.texture_sets.get(ts).cloned() else { return };
    egui::ScrollArea::vertical().id_salt("tss").show(ui, |ui| {
        ui.label(caption("General properties"));
        egui::Grid::new("tss_grid").num_columns(2).spacing(vec2(8.0, 6.0)).show(ui, |ui| {
            ui.label("Name");
            ui.label(RichText::new(&set.name).color(TOKENS.text_strong));
            ui.end_row();
            ui.label("Size");
            egui::ComboBox::from_id_salt("ts_size").selected_text(format!("{}", set.resolution)).show_ui(ui, |ui| {
                for r in [256u32, 512, 1024, 2048, 4096, 8192] {
                    if ui.selectable_label(r == set.resolution, format!("{r}")).clicked() {
                        app.run("textureSet.resolution", json!({"ts": ts, "resolution": r}));
                    }
                }
            });
            ui.end_row();
            ui.label("Shader instance");
            ui.label("Main shader");
            ui.end_row();
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(caption("Channels"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("Add channel", |ui| {
                    for c in Channel::ALL {
                        if !set.has_channel(c) && ui.button(c.label()).clicked() {
                            app.run("textureSet.channel.add", json!({"ts": ts, "channel": c.id()}));
                            ui.close();
                        }
                    }
                });
            });
        });
        for (c, f) in &set.channels {
            ui.horizontal(|ui| {
                ui.label(RichText::new(c.label()).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if *c != Channel::BaseColor && icon_button(ui, ic::X, "Remove channel", false, 18.0).clicked() {
                        app.run("textureSet.channel.remove", json!({"ts": ts, "channel": c.id()}));
                    }
                    ui.label(RichText::new(f.label()).color(TOKENS.text_dim));
                });
            });
        }
        ui.add_space(6.0);
        let mut hs = set.height_strength;
        if slider(ui, "Height to normal strength", &mut hs, 0.0..=10.0, 2).changed() {
            app.run("textureSet.settings", json!({"ts": ts, "heightStrength": hs}));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(caption("Mesh maps"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Bake Mesh Maps").clicked() {
                    app.ui.mode = crate::state::Mode::Baking;
                }
            });
        });
        let baked = app.session.doc.baked.get(&ts);
        for m in BakedMap::ALL {
            let present = baked.is_some_and(|b| b.maps.contains_key(&m));
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
                ui.painter().rect_filled(r, 2.0, if present { mesh_map_color(m) } else { Color32::from_gray(40) });
                ui.vertical(|ui| {
                    ui.label(RichText::new(m.label()).strong());
                    ui.label(
                        RichText::new(if present { format!("{}_{}", set.name, m.id()) } else { "Missing (bake to create)".into() })
                            .size(10.5)
                            .color(TOKENS.text_dim),
                    );
                });
            });
        }
    });
}

fn mesh_map_color(m: BakedMap) -> Color32 {
    match m {
        BakedMap::Normal => Color32::from_rgb(128, 128, 255),
        BakedMap::WorldNormal => Color32::from_rgb(120, 200, 140),
        BakedMap::Id => Color32::from_rgb(220, 60, 60),
        BakedMap::AmbientOcclusion => Color32::from_gray(200),
        BakedMap::Curvature => Color32::from_gray(128),
        BakedMap::Position => Color32::from_rgb(120, 180, 90),
        BakedMap::Thickness => Color32::from_gray(160),
        BakedMap::Height => Color32::from_gray(128),
        BakedMap::BentNormal => Color32::from_rgb(140, 170, 230),
        BakedMap::Opacity => Color32::from_gray(230),
        BakedMap::ColorMap => Color32::from_rgb(200, 150, 90),
        BakedMap::WorldDirection => Color32::from_gray(180),
    }
}

// ---------------------------------------------------------------------------------------------
// Properties

/// The Properties tab title: what the panel edits.
pub(crate) fn properties_title(app: &TextureCraftApp) -> String {
    let p = &app.session.doc.project;
    let what = match (app.session.selected_layer().and_then(|id| p.layers.get(&id)), app.session.target) {
        (Some(_), StackTarget::Effect(e)) => p.effect(e).map(|(_, x)| x.name.clone()).unwrap_or_default(),
        (Some(l), StackTarget::Content) if matches!(l.kind, LayerKind::Fill(_)) => "Fill".to_string(),
        (Some(l), StackTarget::Content) if l.is_group() => "Folder".to_string(),
        (Some(l), StackTarget::Content) if l.instance_source().is_some() => "Instance".to_string(),
        _ => app.session.tool.tool.label().to_string(),
    };
    format!("{} - {what}", crate::i18n::tr("Properties"))
}

pub(crate) fn properties(app: &mut TextureCraftApp, ui: &mut Ui) {
    let sel = app.session.selected_layer().and_then(|id| app.session.doc.project.layers.get(&id).cloned());
    let target = app.session.target;
    egui::ScrollArea::vertical().id_salt("props").auto_shrink([false, false]).show(ui, |ui| match (&sel, target) {
        (Some(l), StackTarget::Effect(e)) => effect_properties(app, ui, l, e),
        (Some(l), StackTarget::Content) if matches!(l.kind, LayerKind::Fill(_)) => fill_properties(app, ui, l),
        (Some(l), StackTarget::Content) if l.is_group() => {
            ui.label(RichText::new("A folder blends its children (pass-through). Set per-channel opacity in the layer rows.").color(TOKENS.text_dim));
        }
        (Some(l), StackTarget::Content) if l.instance_source().is_some() => instance_properties(app, ui, l),
        (Some(l), StackTarget::Mask) => {
            height_mask_section(app, ui, l);
            ui.separator();
            tool_properties(app, ui);
        }
        _ => tool_properties(app, ui),
    });
}

/// An instance draws its source: point there for edits.
fn instance_properties(app: &mut TextureCraftApp, ui: &mut Ui, layer: &Layer) {
    let Some(src) = layer.instance_source() else { return };
    let p = &app.session.doc.project;
    let name = p.layers.get(&src).map_or_else(|| "a missing layer".to_string(), |l| format!("\"{}\"", l.name));
    let set = p.locate(src).and_then(|(ts, _, _)| p.texture_sets.get(ts)).map(|t| t.name.clone()).unwrap_or_default();
    ui.label(caption("Instance"));
    ui.label(RichText::new(format!("{} Draws {name} of texture set \"{set}\". Edits to the source show in every instance.", ic::LINK)).color(TOKENS.text_dim));
    if ui.button("Select source").clicked() {
        app.run("layer.select", json!({"layer": src.0}));
    }
}

/// Mask with height: the layer's height shapes its mask transitions.
fn height_mask_section(app: &mut TextureCraftApp, ui: &mut Ui, layer: &Layer) {
    let id = layer.id.0;
    let hm = app.session.doc.project.content_layer(layer.id).map(|l| l.height_mask).unwrap_or_default();
    section(ui, "sec_height_mask", "Mask with height", true, |ui| {
        let mut on = hm.enabled;
        if ui
            .checkbox(&mut on, "Combine with the layer's height")
            .on_hover_text("Partial mask values follow the layer's height: high parts show first")
            .changed()
        {
            app.run("layer.set", json!({"layer": id, "heightMask": {"enabled": on}}));
        }
        ui.add_enabled_ui(hm.enabled, |ui| {
            let (mut contrast, mut offset) = (hm.contrast, hm.offset);
            if slider(ui, "Contrast", &mut contrast, 0.0..=16.0, 2).drag_stopped() {
                app.run("layer.set", json!({"layer": id, "heightMask": {"contrast": contrast}}));
            }
            if slider(ui, "Height offset", &mut offset, -1.0..=1.0, 2).drag_stopped() {
                app.run("layer.set", json!({"layer": id, "heightMask": {"offset": offset}}));
            }
        });
    });
}

fn tool_properties(app: &mut TextureCraftApp, ui: &mut Ui) {
    // Brush preview: a stroke and a sphere with the tool's material.
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 88.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2.0, Color32::from_rgb(0x1e, 0x1e, 0x1e));
    let half = rect.width() * 0.5;
    let b = &app.session.tool.brush;
    let mut pts = Vec::new();
    for k in 0..=40 {
        let t = k as f32 / 40.0;
        pts.push(pos2(rect.left() + 14.0 + t * (half - 28.0), rect.center().y + (t * 9.0).sin() * 18.0));
    }
    let w = (b.size / 0.25 * 60.0).clamp(2.0, 30.0);
    let a = (b.flow * b.opacity * 255.0) as u8;
    p.add(egui::Shape::line(pts, Stroke::new(w, Color32::from_white_alpha(a.max(30)))));
    let base = app.session.tool.values.get(&Channel::BaseColor).copied().unwrap_or([0.8; 3]);
    crate::thumbs::paint_sphere(p, pos2(rect.left() + half * 1.5, rect.center().y), 36.0, base);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        for (tab, icon, tip) in
            [(PropsTab::Brush, ic::PAINT_BRUSH, "Brush"), (PropsTab::Alpha, ic::CIRCLE_HALF, "Alpha"), (PropsTab::Material, ic::SPHERE, "Material")]
        {
            if icon_button(ui, icon, tip, app.ui.props_tab == tab, 24.0).clicked() {
                app.ui.props_tab = tab;
            }
        }
    });
    ui.separator();
    if app.session.quick_mask.editing {
        ui.label(RichText::new("Editing the quick mask: strokes paint where painting is allowed. Press U to finish.").color(TOKENS.text_dim));
        ui.separator();
    }
    let tool_section: Option<fn(&mut TextureCraftApp, &mut Ui)> = match app.session.tool.tool {
        Tool::Projection => Some(stencil_section),
        Tool::Clone => Some(clone_section),
        Tool::Smudge => Some(smudge_section),
        Tool::Path => Some(path_section),
        _ => None,
    };
    if let Some(section) = tool_section {
        section(app, ui);
        ui.separator();
    }
    match app.ui.props_tab {
        PropsTab::Brush => brush_section(app, ui),
        PropsTab::Alpha => alpha_section(app, ui),
        PropsTab::Material => material_section(app, ui),
    }
}

fn stencil_section(app: &mut TextureCraftApp, ui: &mut Ui) {
    ui.label(caption("Stencil"));
    let names: Vec<String> = app.session.doc.resources.keys().cloned().collect();
    let current = app.session.tool.stencil.as_ref().map(|s| s.image.clone()).unwrap_or_else(|| "None".into());
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("stencil_img").width(170.0).selected_text(current.clone()).show_ui(ui, |ui| {
            if ui.selectable_label(app.session.tool.stencil.is_none(), "None").clicked() {
                app.run("tool.stencil", json!({"clear": true}));
            }
            for n in &names {
                if ui.selectable_label(*n == current, n).clicked() {
                    app.run("tool.stencil", json!({"image": n}));
                }
            }
        });
        import_button(app, ui);
    });
    if names.is_empty() {
        ui.label(RichText::new("Import an image (File > Import resources) to project it onto the mesh.").color(TOKENS.text_dim));
        return;
    }
    if let Some(st) = app.session.tool.stencil.as_mut() {
        slider(ui, "Scale", &mut st.scale, 0.05..=4.0, 2);
        slider(ui, "Rotation", &mut st.rotation, 0.0..=360.0, 0);
        slider(ui, "Overlay opacity", &mut st.opacity, 0.0..=1.0, 2);
        ui.checkbox(&mut st.tile, "Tile");
        ui.label(RichText::new("Hold S: left-drag rotates, right-drag scales, middle-drag moves the stencil.").size(10.5).color(TOKENS.text_dim));
    }
}

fn import_button(app: &mut TextureCraftApp, ui: &mut Ui) {
    if ui.small_button("Import...").clicked() {
        let ctx = ui.ctx().clone();
        crate::menus::invoke(app, &ctx, "resource.import");
    }
}

fn clone_section(app: &mut TextureCraftApp, ui: &mut Ui) {
    ui.label(caption("Clone"));
    let Some(src) = app.session.tool.clone_source else {
        ui.label(RichText::new("Hold V and click the mesh to set the source, then paint elsewhere to copy it.").color(TOKENS.text_dim));
        return;
    };
    ui.horizontal(|ui| {
        ui.label(format!("Source  U {:.3}  V {:.3}", src.uv[0], src.uv[1]));
        if ui.small_button("Clear").clicked() {
            app.run("tool.cloneSource", json!({"clear": true}));
        }
    });
    let hint = if src.offset.is_some() {
        "Aligned: the offset set by the first stroke is kept. V + click sets a new source."
    } else {
        "The first stroke sets the offset from the source."
    };
    ui.label(RichText::new(hint).size(10.5).color(TOKENS.text_dim));
}

fn smudge_section(app: &mut TextureCraftApp, ui: &mut Ui) {
    ui.label(caption("Smudge"));
    let mut v = app.session.tool.smudge_strength * 100.0;
    if slider(ui, "Strength", &mut v, 0.0..=100.0, 0).changed() {
        app.session.tool.smudge_strength = (v / 100.0).clamp(0.0, 1.0);
    }
    ui.label(RichText::new("Drags the layer stack's values along the stroke into the selected layer.").size(10.5).color(TOKENS.text_dim));
}

fn path_section(app: &mut TextureCraftApp, ui: &mut Ui) {
    ui.label(caption("Path"));
    ui.horizontal(|ui| crate::toolbar::path_controls(app, ui));
    let n = app.session.tool.path.points.len();
    let hint = format!(
        "{n} control points. Click the mesh (or the 2D view) to add points and drag them to adjust; click the first point to close the path. Enter paints it with the brush (a closed, filled path fills its inside), Esc cancels."
    );
    ui.label(RichText::new(hint).size(10.5).color(TOKENS.text_dim));
}

fn brush_section(app: &mut TextureCraftApp, ui: &mut Ui) {
    let b = &mut app.session.tool.brush;
    ui.label(caption("Brush"));
    section(ui, "sec_size", "Size", true, |ui| {
        let mut size = b.size / 0.25 * 100.0;
        if slider(ui, "Size", &mut size, 0.05..=100.0, 1).changed() {
            b.size = (size / 100.0 * 0.25).clamp(0.0005, 0.25);
        }
        let mut ms = b.min_size * 100.0;
        if slider(ui, "Minimum size (%)", &mut ms, 0.0..=100.0, 0).changed() {
            b.min_size = ms / 100.0;
        }
        ui.checkbox(&mut b.screen_space, "Size in screen space");
    });
    section(ui, "sec_flow", "Flow and opacity", true, |ui| {
        let mut f = b.flow * 100.0;
        if slider(ui, "Flow", &mut f, 0.0..=100.0, 0).changed() {
            b.flow = f / 100.0;
        }
        let mut o = b.opacity * 100.0;
        if slider(ui, "Stroke opacity", &mut o, 0.0..=100.0, 0).changed() {
            b.opacity = o / 100.0;
        }
        let mut s = b.spacing * 100.0;
        if slider(ui, "Spacing", &mut s, 1.0..=1000.0, 0).changed() {
            b.spacing = s / 100.0;
        }
        let mut h = b.hardness * 100.0;
        if slider(ui, "Hardness", &mut h, 0.0..=99.9, 0).changed() {
            b.hardness = h / 100.0;
        }
    });
    section(ui, "sec_angle", "Angle", false, |ui| {
        slider(ui, "Angle", &mut b.angle, -180.0..=180.0, 0);
        ui.checkbox(&mut b.follow_path, "Follow path");
    });
    section(ui, "sec_jitter", "Jitters", false, |ui| {
        for (label, v) in [("Size jitter", &mut b.jitter_size), ("Angle jitter", &mut b.jitter_angle), ("Flow jitter", &mut b.jitter_flow)] {
            let mut x = *v * 100.0;
            if slider(ui, label, &mut x, 0.0..=100.0, 0).changed() {
                *v = x / 100.0;
            }
        }
        let mut pj = b.jitter_position * 100.0;
        if slider(ui, "Position jitter", &mut pj, 0.0..=400.0, 0).changed() {
            b.jitter_position = pj / 100.0;
        }
    });
    section(ui, "sec_align", "Alignment and culling", false, |ui| {
        egui::ComboBox::from_label("Alignment").selected_text(format!("{:?}", b.alignment)).show_ui(ui, |ui| {
            ui.selectable_value(&mut b.alignment, texturecraft_engine::paint::Alignment::Tangent, "Tangent | Wrap");
            ui.selectable_value(&mut b.alignment, texturecraft_engine::paint::Alignment::Camera, "Camera");
        });
        ui.checkbox(&mut b.backface_culling, "Backface culling");
        slider(ui, "Angle fade from", &mut b.angle_fade.0, 0.0..=90.0, 0);
        slider(ui, "Angle fade to", &mut b.angle_fade.1, 0.0..=90.0, 0);
        slider(ui, "Stamp depth", &mut b.depth, 0.05..=4.0, 2);
    });
    section(ui, "sec_particles", "Particles", false, |ui| {
        let pt = &mut b.particles;
        ui.checkbox(&mut pt.enabled, "Particle brush (slides down the surface)");
        let mut count = pt.count as f32;
        if slider(ui, "Count per stamp", &mut count, 1.0..=16.0, 0).changed() {
            pt.count = count as u32;
        }
        let mut life = pt.lifetime as f32;
        if slider(ui, "Lifetime (steps)", &mut life, 1.0..=128.0, 0).changed() {
            pt.lifetime = life as u32;
        }
        slider(ui, "Gravity", &mut pt.gravity, 0.0..=2.0, 2);
    });
    section(ui, "sec_lazy", "Lazy mouse and symmetry", false, |ui| {
        slider(ui, "Lazy mouse distance", &mut b.lazy, 0.0..=200.0, 0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut b.symmetry.x, "Mirror X");
            ui.checkbox(&mut b.symmetry.y, "Mirror Y");
            ui.checkbox(&mut b.symmetry.z, "Mirror Z");
        });
    });
    *b = b.sanitized();
}

/// One alpha in the picker: a 52×64 tile with the shape drawn by `paint` and a label.
fn alpha_tile(ui: &mut Ui, on: bool, label: &str, paint: impl FnOnce(&egui::Painter, Rect)) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(52.0, 64.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, if on { Color32::from_rgb(0x34, 0x3c, 0x48) } else { Color32::from_rgb(0x1e, 0x1e, 0x1e) });
    if on {
        p.rect_stroke(rect, 3.0, Stroke::new(1.0, TOKENS.accent), egui::StrokeKind::Inside);
    }
    paint(p, Rect::from_center_size(rect.center() - vec2(0.0, 7.0), vec2(40.0, 40.0)));
    p.text(rect.center_bottom() - vec2(0.0, 6.0), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(9.5), TOKENS.text);
    resp.on_hover_text(label)
}

fn alpha_section(app: &mut TextureCraftApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label(caption("Alpha"));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| import_button(app, ui));
    });
    let brush = &app.session.tool.brush;
    let (current, image, hardness) = (brush.alpha, brush.alpha_image.clone(), brush.hardness);
    let library = if image.is_none() { brush.alpha_library.as_deref().and_then(alphas::find) } else { None };
    let mut pick = None;
    let zone = ui.horizontal_wrapped(|ui| {
        for a in BrushAlpha::ALL {
            let on = image.is_none() && library.is_none() && current == a;
            if alpha_tile(ui, on, a.label(), |p, r| crate::thumbs::paint_alpha(p, r, a, hardness)).clicked() {
                pick = Some(json!(a.label()));
            }
        }
        // The library alpha in use (picked from the shelf's Alphas tab).
        if let Some(a) = library {
            let tex = app.thumbs.alpha(ui.ctx(), a);
            alpha_tile(ui, true, &a.label, |p, r| {
                if let Some(t) = tex {
                    p.image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
            });
        }
        // Imported images, sampled as grayscale.
        for name in app.session.doc.resources.keys() {
            let Some(img) = app.session.images.get(name) else { continue };
            let tex = app.thumbs.image(ui.ctx(), name, img);
            let on = image.as_deref() == Some(name.as_str());
            if alpha_tile(ui, on, name, |p, r| {
                p.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            })
            .clicked()
            {
                pick = Some(json!({ "image": name }));
            }
        }
    });
    if let Some(alpha) = pick {
        app.run("tool.brush", json!({ "alpha": alpha }));
    }
    ui.label(RichText::new("More in Assets > Alphas: click one, or drag it here").size(10.0).color(TOKENS.text_dim));
    crate::shelf::drop_target(app, ui, &zone.response, false);
}

fn material_section(app: &mut TextureCraftApp, ui: &mut Ui) {
    ui.label(caption("Material"));
    let chans = app.session.doc.project.texture_set(app.session.ts()).map(|t| t.channel_list()).unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        for c in &chans {
            let on = app.session.tool.enabled.contains(c);
            if chip(ui, c.short(), on).clicked() {
                app.run("tool.material", json!({"channel": c.id(), "enabled": !on}));
            }
        }
    });
    ui.add_space(4.0);
    let mask_mode = !targets_content(app);
    if mask_mode {
        let mut v = app.session.tool.mask_value;
        if slider(ui, "Mask value (gray)", &mut v, 0.0..=1.0, 2).changed() {
            app.session.tool.mask_value = v;
        }
        return;
    }
    for c in chans {
        if !app.session.tool.enabled.contains(&c) {
            continue;
        }
        let mut v = app.session.tool.values.get(&c).copied().unwrap_or(c.default_value());
        match c.kind() {
            texturecraft_engine::core::ChannelKind::Color => {
                ui.horizontal(|ui| {
                    ui.label(c.label());
                    if color_swatch(ui, &format!("tool_{}", c.id()), &mut v, ui.available_width() - 4.0) {
                        app.session.tool.values.insert(c, v);
                    }
                });
            }
            texturecraft_engine::core::ChannelKind::NormalMap => {
                ui.label(RichText::new("Normal: painting uses the flat normal").color(TOKENS.text_dim));
            }
            texturecraft_engine::core::ChannelKind::Signed => {
                if slider(ui, c.label(), &mut v[0], -1.0..=1.0, 2).changed() {
                    app.session.tool.values.insert(c, [v[0]; 3]);
                }
            }
            texturecraft_engine::core::ChannelKind::Scalar => {
                if slider(ui, c.label(), &mut v[0], 0.0..=1.0, 2).changed() {
                    app.session.tool.values.insert(c, [v[0]; 3]);
                }
            }
        }
    }
    if app.session.tool.tool == Tool::Eraser {
        ui.label(RichText::new("The eraser removes paint from the enabled channels.").color(TOKENS.text_dim));
    }
}

fn fill_properties(app: &mut TextureCraftApp, ui: &mut Ui, layer: &Layer) {
    let LayerKind::Fill(fill) = &layer.kind else { return };
    let on: Vec<Channel> = layer.channels.keys().copied().collect();
    ui.label(caption("Fill"));
    fill_editor(app, ui, fill, &json!({"layer": layer.id.0}), &on, layer.id);
}

/// `base` with the fields of `extra` added.
fn with(base: &Value, extra: Value) -> Value {
    let mut v = base.clone();
    if let (Some(o), Value::Object(e)) = (v.as_object_mut(), extra) {
        o.extend(e);
    }
    v
}

/// Anchor points below layer `reader` in its texture set: (effect id, "layer / anchor" label).
fn anchors_below(app: &TextureCraftApp, reader: LayerId) -> Vec<(LayerId, String)> {
    let p = &app.session.doc.project;
    let Some((ts, _, _)) = p.locate(reader) else { return Vec::new() };
    p.walk(ts)
        .into_iter()
        .map(|(id, _)| id)
        .take_while(|id| *id != reader)
        .filter_map(|id| p.layers.get(&id))
        .flat_map(|l| l.content_effects.iter().filter(|e| matches!(e.kind, EffectKind::Anchor)).map(move |e| (e.id, format!("{} / {}", l.name, e.name))))
        .collect()
}

fn anchor_label(app: &TextureCraftApp, anchor: LayerId, channel: Channel) -> String {
    let name = app.session.doc.project.effect(anchor).map(|(_, e)| e.name.clone()).unwrap_or_else(|| "missing".into());
    format!("Anchor: {name} ({})", channel.label())
}

/// Projection and per-channel sources of a fill: a fill layer's (`base` = {"layer"}, `on` =
/// its enabled channels) or a content fill effect's (`base` = {"effect"}, `on` = the channels
/// it has a source for). `reader` is the layer owning the fill (for anchor sources).
fn fill_editor(app: &mut TextureCraftApp, ui: &mut Ui, fill: &Fill, base: &Value, on: &[Channel], reader: LayerId) {
    use texturecraft_engine::core::{ChannelKind, Projection};
    let salt = base.to_string();
    let proj_label = fill.projection.label();
    egui::ComboBox::from_label("Projection").selected_text(proj_label).show_ui(ui, |ui| {
        for (id, label) in [
            ("uv", "UV projection"),
            ("triplanar", "Tri-planar projection"),
            ("planar", "Planar projection"),
            ("spherical", "Spherical projection"),
            ("cylindrical", "Cylindrical projection"),
        ] {
            if ui.selectable_label(proj_label == label, label).clicked() {
                app.run("layer.fill.projection", with(base, json!({"projection": id})));
            }
        }
    });
    let (mut scale, proj_id) = match &fill.projection {
        Projection::Uv { scale, .. } => (scale[0], "uv"),
        Projection::Triplanar { scale, .. } => (*scale, "triplanar"),
        Projection::Planar { scale, .. } => (*scale, "planar"),
        Projection::Spherical { scale } => (*scale, "spherical"),
        Projection::Cylindrical { scale, .. } => (*scale, "cylindrical"),
    };
    if slider(ui, "Scale / tiling", &mut scale, 0.01..=20.0, 2).drag_stopped() {
        app.run("layer.fill.projection", with(base, json!({"projection": proj_id, "scale": scale})));
    }
    ui.add_space(6.0);
    ui.label(caption("Material"));
    let chans = app.session.doc.project.texture_set(app.session.ts()).map(|t| t.channel_list()).unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        for c in &chans {
            let enabled = on.contains(c);
            if chip(ui, c.short(), enabled).clicked() {
                app.run("layer.fill.set", with(base, json!({"channel": c.id(), "enabled": !enabled})));
            }
        }
    });
    ui.add_space(4.0);
    let anchors = anchors_below(app, reader);
    for c in chans {
        if !on.contains(&c) {
            continue;
        }
        let set = |extra: Value| with(base, with(&json!({"channel": c.id()}), extra));
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(c.label()).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.menu_button(RichText::new(ic::DOTS_THREE).size(13.0), |ui| {
                        if ui.button("Uniform value").clicked() {
                            app.run("layer.fill.set", set(json!({"value": c.default_value()})));
                            ui.close();
                        }
                        ui.separator();
                        for k in ProceduralKind::ALL {
                            if ui.button(k.label()).clicked() {
                                let (lo, hi) = match c {
                                    Channel::BaseColor => ([0.15, 0.15, 0.15], [0.85, 0.85, 0.85]),
                                    Channel::Height => ([-0.05; 3], [0.05; 3]),
                                    _ => ([0.2; 3], [0.8; 3]),
                                };
                                app.run("layer.fill.set", set(json!({"procedural": k.label(), "low": lo, "high": hi})));
                                ui.close();
                            }
                        }
                        if !anchors.is_empty() {
                            ui.separator();
                        }
                        for (a, label) in &anchors {
                            if ui.button(format!("Anchor: {label}")).clicked() {
                                app.run("layer.fill.set", set(json!({"anchor": a.0})));
                                ui.close();
                            }
                        }
                    })
                    .response
                    .on_hover_text("Channel source: uniform value, procedural or anchor");
                });
            });
            match fill.source(c) {
                Source::Uniform(mut v) => {
                    if c.kind() == ChannelKind::Color {
                        if color_swatch(ui, &format!("fill_{salt}_{}", c.id()), &mut v, ui.available_width() - 4.0) {
                            app.run("layer.fill.set", set(json!({"color": v})));
                        }
                    } else {
                        let range = if c.kind() == ChannelKind::Signed { -1.0..=1.0 } else { 0.0..=1.0 };
                        let r = slider(ui, "Value", &mut v[0], range, 2);
                        if r.changed() {
                            app.run("layer.fill.set", set(json!({"value": v[0]})));
                        }
                    }
                }
                Source::Procedural { pattern, low, high } => {
                    ui.label(RichText::new(format!("{} (scale {:.1}, seed {})", pattern.kind.label(), pattern.scale, pattern.seed)).color(TOKENS.text_dim));
                    let mut s = pattern.scale;
                    let mut con = pattern.contrast;
                    let r1 = slider(ui, "Pattern scale", &mut s, 0.1..=100.0, 1);
                    let r2 = slider(ui, "Contrast", &mut con, 0.0..=8.0, 2);
                    let mut seed = pattern.seed as f32;
                    let r3 = slider(ui, "Seed", &mut seed, 0.0..=999.0, 0);
                    if r1.drag_stopped() || r2.drag_stopped() || r3.drag_stopped() {
                        app.run("layer.fill.set", set(json!({"procedural": pattern.kind.label(), "scale": s, "contrast": con, "seed": seed as u64, "low": low, "high": high})));
                    }
                    if c.kind() == ChannelKind::Color {
                        let (mut lo, mut hi) = (low, high);
                        let mut changed = false;
                        ui.horizontal(|ui| {
                            ui.label("Low");
                            changed |= color_swatch(ui, &format!("lo_{salt}_{}", c.id()), &mut lo, 60.0);
                            ui.label("High");
                            changed |= color_swatch(ui, &format!("hi_{salt}_{}", c.id()), &mut hi, 60.0);
                        });
                        if changed {
                            app.run("layer.fill.set", set(json!({"procedural": pattern.kind.label(), "scale": s, "contrast": con, "seed": pattern.seed, "low": lo, "high": hi})));
                        }
                    }
                }
                Source::Image { name } => {
                    ui.label(format!("Image: {name}"));
                }
                Source::Anchor { anchor, channel } => {
                    ui.label(RichText::new(anchor_label(app, anchor, channel)).color(TOKENS.text_dim));
                }
            }
        });
    }
}

fn effect_properties(app: &mut TextureCraftApp, ui: &mut Ui, layer: &Layer, eid: LayerId) {
    let Some((_, e)) = app.session.doc.project.effect(eid).map(|(l, e)| (l, e.clone())) else { return };
    let id = eid.0;
    ui.horizontal(|ui| {
        ui.label("Blending");
        egui::ComboBox::from_id_salt("eff_blend").selected_text(e.blend.label()).show_ui(ui, |ui| {
            for m in BlendMode::all() {
                if ui.selectable_label(m == e.blend, m.label()).clicked() {
                    app.run("effect.set", json!({"effect": id, "blend": format!("{m:?}")}));
                }
            }
        });
    });
    let mut op = e.opacity;
    if slider(ui, "Opacity", &mut op, 0.0..=1.0, 2).drag_stopped() {
        app.run("effect.set", json!({"effect": id, "opacity": op}));
    }
    ui.separator();
    match &e.kind {
        EffectKind::Generator(g) => {
            ui.label(caption("Generator"));
            egui::ComboBox::from_id_salt("gen_kind").selected_text(g.kind.label()).show_ui(ui, |ui| {
                for k in GeneratorKind::ALL {
                    if ui.selectable_label(k == g.kind, k.label()).clicked() {
                        // Replace the generator kind: remove and add keeps ordering simple.
                        app.run("effect.remove", json!({"effect": id}));
                        app.run("effect.add.generator", json!({"generator": format!("{k:?}")}));
                    }
                }
            });
            let (mut level, mut soft, mut noise, mut ns) = (g.level, g.softness, g.noise_amount, g.noise_scale);
            let r1 = slider(ui, "Level", &mut level, 0.0..=1.0, 2);
            let r2 = slider(ui, "Softness", &mut soft, 0.0..=1.0, 2);
            let r3 = slider(ui, "Noise amount", &mut noise, 0.0..=2.0, 2);
            let r4 = slider(ui, "Noise scale", &mut ns, 0.1..=64.0, 1);
            let mut inv = g.invert;
            let r5 = ui.checkbox(&mut inv, "Invert");
            if r1.drag_stopped() || r2.drag_stopped() || r3.drag_stopped() || r4.drag_stopped() || r5.changed() {
                app.run("effect.set", json!({"effect": id, "level": level, "softness": soft, "noise": noise, "noiseScale": ns, "invert": inv}));
            }
            if !app.session.doc.baked.contains_key(&app.session.ts()) {
                ui.label(
                    RichText::new(format!("{} Bake mesh maps (F8) for curvature, AO and thickness based generators.", ic::WARNING))
                        .color(Color32::from_rgb(0xe0, 0xb0, 0x40)),
                );
            }
        }
        EffectKind::Levels(l) => {
            ui.label(caption("Levels"));
            let mut v = [l.in_low, l.in_high, l.gamma, l.out_low, l.out_high];
            let labels = ["Input low", "Input high", "Gamma", "Output low", "Output high"];
            let mut done = false;
            for (i, label) in labels.iter().enumerate() {
                let range = if i == 2 { 0.1..=5.0 } else { 0.0..=1.0 };
                done |= slider(ui, label, &mut v[i], range, 2).drag_stopped();
            }
            if done {
                app.run("effect.set", json!({"effect": id, "inLow": v[0], "inHigh": v[1], "gamma": v[2], "outLow": v[3], "outHigh": v[4]}));
            }
        }
        EffectKind::Fill { source, .. } => {
            ui.label(caption("Fill"));
            let anchors = anchors_below(app, layer.id);
            let current = match source {
                Source::Anchor { anchor, channel } => anchor_label(app, *anchor, *channel),
                Source::Uniform(_) => "Uniform value".to_string(),
                Source::Procedural { pattern, .. } => pattern.kind.label().to_string(),
                Source::Image { name } => format!("Image: {name}"),
            };
            egui::ComboBox::from_label("Source").selected_text(current).show_ui(ui, |ui| {
                if ui.selectable_label(matches!(source, Source::Uniform(_)), "Uniform value").clicked() {
                    app.run("effect.set", json!({"effect": id, "value": 1.0}));
                }
                for (a, label) in &anchors {
                    if ui.selectable_label(source.anchor() == Some(*a), format!("Anchor: {label}")).clicked() {
                        app.run("effect.set", json!({"effect": id, "anchor": a.0}));
                    }
                }
            });
            match source {
                Source::Uniform(v) => {
                    let mut x = v[0];
                    if slider(ui, "Value", &mut x, 0.0..=1.0, 2).drag_stopped() {
                        app.run("effect.set", json!({"effect": id, "value": x}));
                    }
                }
                Source::Anchor { channel, .. } => {
                    let chans = app.session.doc.project.texture_set(app.session.ts()).map(|t| t.channel_list()).unwrap_or_default();
                    egui::ComboBox::from_label("Channel").selected_text(channel.label()).show_ui(ui, |ui| {
                        for c in chans {
                            if ui.selectable_label(c == *channel, c.label()).clicked() {
                                app.run("effect.set", json!({"effect": id, "anchorChannel": c.id()}));
                            }
                        }
                    });
                    ui.label(RichText::new("Colors read as gray. Add a levels or blur effect above to shape the result.").color(TOKENS.text_dim));
                }
                _ => {}
            }
            if anchors.is_empty() {
                ui.label(RichText::new("Add an anchor point to a layer below to use its stack as a mask.").color(TOKENS.text_dim));
            }
        }
        EffectKind::Material(f) => {
            ui.label(caption("Fill"));
            let on: Vec<Channel> = f.sources.keys().copied().collect();
            fill_editor(app, ui, f, &json!({"effect": id}), &on, layer.id);
        }
        EffectKind::Anchor => {
            ui.label(caption("Anchor point"));
            ui.label(
                RichText::new("Records the stack up to this layer. Fill effects in masks (and fill layers) above can read it as a source.")
                    .color(TOKENS.text_dim),
            );
        }
        EffectKind::Paint => {
            let what = if layer.content_effects.iter().any(|x| x.id == eid) { "the layer content" } else { "this mask. Use the Material tab's mask value for gray" };
            ui.label(RichText::new(format!("Paint on the mesh to edit {what}.")).color(TOKENS.text_dim));
            tool_properties(app, ui);
        }
        EffectKind::Filter(Filter::Hsl { hue, saturation, lightness }) => {
            ui.label(caption("HSL"));
            let (mut h, mut s, mut l) = (hue * 360.0, *saturation, *lightness);
            let r1 = slider(ui, "Hue shift (degrees)", &mut h, -180.0..=180.0, 0);
            let r2 = slider(ui, "Saturation", &mut s, -1.0..=1.0, 2);
            let r3 = slider(ui, "Lightness", &mut l, -1.0..=1.0, 2);
            if r1.drag_stopped() || r2.drag_stopped() || r3.drag_stopped() {
                app.run("effect.set", json!({"effect": id, "hue": h / 360.0, "saturation": s, "lightness": l}));
            }
        }
        EffectKind::Filter(Filter::Blur { radius }) => {
            ui.label(caption("Blur"));
            let mut r = *radius;
            if slider(ui, "Intensity (texels at 2K)", &mut r, 0.0..=128.0, 1).drag_stopped() {
                app.run("effect.set", json!({"effect": id, "radius": r}));
            }
        }
        EffectKind::Filter(Filter::Sharpen { amount }) => {
            ui.label(caption("Sharpen"));
            let mut a = *amount;
            if slider(ui, "Amount", &mut a, 0.0..=5.0, 2).drag_stopped() {
                app.run("effect.set", json!({"effect": id, "amount": a}));
            }
        }
        EffectKind::Filter(Filter::Threshold { level }) => {
            ui.label(caption("Threshold"));
            let mut l = *level;
            if slider(ui, "Level", &mut l, 0.0..=1.0, 2).drag_stopped() {
                app.run("effect.set", json!({"effect": id, "level": l}));
            }
        }
        other => {
            ui.label(RichText::new(other.name()).color(TOKENS.text_dim));
        }
    }
}

