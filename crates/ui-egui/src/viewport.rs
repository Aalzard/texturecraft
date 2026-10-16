//! The 3D viewport and the 2D UV view: navigation, painting input, brush cursor and the GPU
//! callbacks that render them.

use std::sync::Mutex;

use egui::{Color32, PointerButton, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;
use texturecraft_engine::camera_cmds::DEFAULT_CAMERA;
use texturecraft_engine::core::{BakedMap, Channel};
use texturecraft_engine::geom::glam::{Vec2, Vec3};
use texturecraft_engine::{Tool, ViewMode};
use texturecraft_render::{Frame, Shade, ViewKind};

use crate::TextureCraftApp;
use crate::gpu::ViewCallback;
use crate::state::ViewLayout;
use crate::theme::TOKENS;

pub const VIEW_3D: u64 = 1;
pub const VIEW_2D: u64 = 2;

/// While on, a click in the 3D view focuses the depth of field there (a transient UI mode kept in
/// egui's memory).
fn focus_pick_id() -> egui::Id {
    egui::Id::new("tc_focus_pick")
}

pub fn focus_picking(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(focus_pick_id())).unwrap_or(false)
}

pub fn set_focus_picking(ctx: &egui::Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(focus_pick_id(), on));
}

pub fn central(app: &mut TextureCraftApp, ui: &mut Ui) {
    let aux = match app.session.view {
        _ if app.session.quick_mask.editing => Some(crate::gpu::AuxView::QuickMask),
        ViewMode::MeshMap(m) => Some(crate::gpu::AuxView::MeshMap(m)),
        ViewMode::Mask(id) => Some(crate::gpu::AuxView::Mask(id)),
        _ => None,
    };
    let mut sync = PendingSync(Some(aux));
    let full = ui.max_rect();
    let mode = app.ui.mode;
    path_keys(app, ui);
    let layout = if mode == crate::state::Mode::Painting { app.ui.layout } else { ViewLayout::Only3D };
    match layout {
        ViewLayout::Only3D => view_3d(app, ui, full, &mut sync),
        ViewLayout::Only2D => view_2d(app, ui, full, &mut sync),
        ViewLayout::Split => {
            let mid = full.center().x;
            let left = Rect::from_min_max(full.min, pos2(mid - 1.0, full.max.y));
            let right = Rect::from_min_max(pos2(mid + 1.0, full.min.y), full.max);
            view_3d(app, ui, left, &mut sync);
            view_2d(app, ui, right, &mut sync);
            ui.painter().line_segment([pos2(mid, full.top()), pos2(mid, full.bottom())], Stroke::new(2.0, TOKENS.separator));
        }
    }
    if mode == crate::state::Mode::Rendering {
        crate::render_mode::show(app, ui, full);
    } else if app.render.is_active() {
        crate::render_mode::stop(app);
    }
    // Keep drawing while the 3D view accumulates anti-aliasing samples (it stops on its own).
    let accumulating = layout != ViewLayout::Only2D
        && app
            .render_state
            .as_ref()
            .is_some_and(|rs| rs.renderer.read().callback_resources.get::<crate::gpu::GpuState>().is_some_and(|g| g.renderer.accumulating(VIEW_3D)));
    if app.session.stroke_active() || accumulating {
        ui.ctx().request_repaint();
    }
}

fn shade(app: &TextureCraftApp) -> Shade {
    let (view_mode, channel) = match app.session.view {
        _ if app.session.quick_mask.editing => (2, 0),
        ViewMode::Material => (0, 0),
        ViewMode::Channel(c) => (1, c.index() as u32),
        ViewMode::MeshMap(_) | ViewMode::Mask(_) => (2, 0),
        ViewMode::UvChecker => (3, 0),
    };
    let ts = app.session.ts();
    let hs = app.session.doc.project.texture_sets.get(ts).map(|t| t.atlas_height_strength()).unwrap_or(1.0);
    Shade {
        height_strength: hs,
        view_mode,
        channel,
        wireframe: app.ui.uv_wireframe,
        wireframe_3d: app.ui.wireframe_3d,
        ..Shade::from_project(&app.session.doc.project)
    }
}

/// This frame's scene sync (compositing and GPU uploads), run by the first view drawn, after
/// that view handled its input: a stroke shows in the frame it is painted, not the next.
pub struct PendingSync(Option<Option<crate::gpu::AuxView>>);

fn add_callback(app: &mut TextureCraftApp, ui: &Ui, rect: Rect, frame: Frame, sync: &mut PendingSync) {
    let scene = match (sync.0.take(), app.gpu.as_mut()) {
        (Some(aux), Some(g)) => {
            let t0 = crate::now_ms();
            let scene = g.sync(&mut app.session, aux);
            app.profiler.sync_ms += (crate::now_ms() - t0) as f32;
            Some(scene)
        }
        _ => None,
    };
    let Some(gpu) = &app.gpu else {
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "No GPU renderer available", egui::FontId::proportional(14.0), TOKENS.text_dim);
        return;
    };
    let cb = ViewCallback { frame, sync: Mutex::new(scene), rect, lost: gpu.lost.clone() };
    ui.painter().add(eframe::egui_wgpu::Callback::new_paint_callback(rect, cb));
}

/// The view-mode dropdown shown at the top of each view.
fn view_header(app: &mut TextureCraftApp, ui: &mut Ui, rect: Rect, id: &str) {
    let header = Rect::from_min_size(rect.min + vec2(rect.width() * 0.5 - 150.0, 8.0), vec2(300.0, 22.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(header).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let current = texturecraft_engine::camera_cmds::current(&app.session);
    let names: Vec<String> = app.session.doc.project.cameras.iter().map(|c| c.name.clone()).collect();
    let shown = current.and_then(|i| names.get(i)).map_or(DEFAULT_CAMERA, String::as_str);
    egui::ComboBox::from_id_salt(format!("{id}_cam")).width(130.0).selected_text(shown).show_ui(&mut child, |ui| {
        if ui.selectable_label(current.is_none(), DEFAULT_CAMERA).clicked() {
            app.run("camera.select", json!({}));
        }
        for (i, name) in names.iter().enumerate() {
            if ui.selectable_label(current == Some(i), name).clicked() {
                app.run("camera.select", json!({"index": i}));
            }
        }
        ui.separator();
        if ui.selectable_label(false, "Save view as camera").clicked() {
            app.run("camera.add", json!({}));
        }
    });
    let label = app.session.view.label();
    let channels = app.session.doc.project.texture_set(app.session.ts()).map(|t| t.channel_list()).unwrap_or_default();
    egui::ComboBox::from_id_salt(format!("{id}_mode")).width(130.0).selected_text(label).show_ui(&mut child, |ui| {
        if ui.selectable_label(app.session.view == ViewMode::Material, "Material").clicked() {
            app.run("viewport.view", json!({"mode": "material"}));
        }
        ui.separator();
        for c in channels {
            if ui.selectable_label(app.session.view == ViewMode::Channel(c), c.label()).clicked() {
                app.run("viewport.view", json!({"mode": c.id()}));
            }
        }
        ui.separator();
        for m in BakedMap::ALL {
            if ui.selectable_label(app.session.view == ViewMode::MeshMap(m), format!("Mesh map: {}", m.label())).clicked() {
                app.run("viewport.view", json!({"mode": format!("map:{}", m.id())}));
            }
        }
        ui.separator();
        if ui.selectable_label(app.session.view == ViewMode::UvChecker, "UV checker").clicked() {
            app.run("viewport.view", json!({"mode": "uvChecker"}));
        }
    });
}

fn axis_gizmo(ui: &Ui, rect: Rect, app: &TextureCraftApp, uv: bool) {
    let o = pos2(rect.left() + 28.0, rect.bottom() - 28.0);
    let p = ui.painter();
    if uv {
        p.line_segment([o, o + vec2(16.0, 0.0)], Stroke::new(1.5, Color32::from_rgb(0xd8, 0x4a, 0x3a)));
        p.line_segment([o, o - vec2(0.0, 16.0)], Stroke::new(1.5, Color32::from_rgb(0x5c, 0xc0, 0x4a)));
        p.text(o + vec2(20.0, 0.0), egui::Align2::LEFT_CENTER, "U", egui::FontId::proportional(9.0), TOKENS.text_dim);
        p.text(o - vec2(0.0, 22.0), egui::Align2::CENTER_CENTER, "V", egui::FontId::proportional(9.0), TOKENS.text_dim);
        return;
    }
    let view = app.session.camera.view();
    for (axis, color, name) in [
        (Vec3::X, Color32::from_rgb(0xd8, 0x4a, 0x3a), "X"),
        (Vec3::Y, Color32::from_rgb(0x5c, 0xc0, 0x4a), "Y"),
        (Vec3::Z, Color32::from_rgb(0x4a, 0x80, 0xe0), "Z"),
    ] {
        let d = view.transform_vector3(axis);
        let end = o + vec2(d.x, -d.y) * 16.0;
        p.line_segment([o, end], Stroke::new(1.5, color));
        p.text(end + vec2(d.x, -d.y) * 6.0, egui::Align2::CENTER_CENTER, name, egui::FontId::proportional(9.0), TOKENS.text_dim);
    }
}

fn view_3d(app: &mut TextureCraftApp, ui: &mut Ui, rect: Rect, sync: &mut PendingSync) {
    let resp = ui.interact(rect, ui.id().with("view3d"), Sense::click_and_drag());
    app.viewport_rect = rect;
    let size = rect.size();
    app.session.viewport = Vec2::new(size.x.max(16.0), size.y.max(16.0));
    let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
    let local = |p: egui::Pos2| Vec2::new(p.x - rect.left(), p.y - rect.top());
    let navigating = alt || ui.input(|i| i.pointer.middle_down() || i.pointer.secondary_down());

    // Navigation (S + drag manipulates the projection stencil instead, below).
    let delta = ui.input(|i| i.pointer.delta());
    let s_held = ui.input(|i| i.key_down(egui::Key::S)) && !ui.ctx().egui_wants_keyboard_input();
    let stencil_drag = s_held && app.session.tool.stencil.is_some();
    if stencil_drag {
        // The stencil moves, turns and scales instead of the camera (handled below).
    } else if resp.dragged_by(PointerButton::Middle) {
        app.session.camera.pan(delta.x / size.y, delta.y / size.y);
    } else if alt && resp.dragged_by(PointerButton::Primary) {
        app.session.camera.orbit(delta.x * 0.008, delta.y * 0.008);
    } else if resp.dragged_by(PointerButton::Secondary) {
        if alt {
            app.session.camera.zoom(-delta.y * 0.01);
        } else if shift {
            // Shift + right drag turns the environment light (Painter's shortcut).
            let d = &mut app.session.doc.project.display;
            d.env_rotation = (d.env_rotation + delta.x * 0.5).rem_euclid(360.0);
        } else {
            app.session.camera.orbit(delta.x * 0.008, delta.y * 0.008);
        }
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            if shift {
                let b = &mut app.session.tool.brush;
                b.size = (b.size * (1.0 + scroll * 0.002)).clamp(0.0005, 0.25);
            } else {
                app.session.camera.zoom(scroll * 0.0025);
            }
        }
    }

    // Stencil: S + left drag rotates, S + right drag scales, S + middle drag moves.
    if s_held && let Some(st) = app.session.tool.stencil.as_mut() {
        if resp.dragged_by(PointerButton::Primary) {
            st.rotation = (st.rotation + delta.x * 0.4).rem_euclid(360.0);
        } else if resp.dragged_by(PointerButton::Secondary) {
            st.scale = (st.scale * (1.0 + delta.x * 0.005)).clamp(0.05, 10.0);
        } else if resp.dragged_by(PointerButton::Middle) {
            st.center[0] += delta.x / size.x * 2.0;
            st.center[1] -= delta.y / size.y * 2.0;
        }
    }
    // Clone source: V + click.
    let v_held = clone_source_key(app, ui);
    if v_held
        && !alt
        && (resp.clicked_by(PointerButton::Primary) || resp.drag_started_by(PointerButton::Primary))
        && let Some(p) = resp.interact_pointer_pos()
    {
        let (q, vp) = (local(p), app.session.viewport);
        app.run("tool.cloneSource", json!({"x": q.x, "y": q.y, "viewport": [vp.x, vp.y]}));
    }
    // Depth of field focus pick: a click focuses there instead of painting.
    let focus_pick = focus_picking(ui.ctx());
    if focus_pick {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if !alt
            && resp.clicked_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            let (q, vp) = (local(p), app.session.viewport);
            app.run("viewport.focusAt", json!({"x": q.x, "y": q.y, "viewport": [vp.x, vp.y]}));
            set_focus_picking(ui.ctx(), false);
        }
    }
    // Decal placement: a click moves the decal being placed there.
    let placing = app.placing_decal.filter(|_| app.ui.mode == crate::state::Mode::Painting);
    if let Some(layer) = placing {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        if !alt
            && resp.clicked_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            let (q, vp) = (local(p), app.session.viewport);
            app.run("layer.decal.set", json!({"layer": layer.0, "x": q.x, "y": q.y, "viewport": [vp.x, vp.y]}));
            app.placing_decal = None;
        }
    }
    // Painting.
    let painting = app.ui.mode == crate::state::Mode::Painting && !alt && !s_held && !v_held && !focus_pick && placing.is_none();
    let path_tool = app.session.tool.tool == Tool::Path;
    let vp = app.session.viewport;
    let show = |app: &TextureCraftApp, p: Vec3| project(app, rect, p);
    if painting && path_tool {
        path_input(app, ui, &resp, false, |app, p| app.session.pick(local(p), vp).map(|h| h.pos), show);
    } else if painting {
        // Pen pressure arrives as touch force (Windows Ink, Wayland, web); mice report none.
        // shortcut: egui 0.36 / winit 0.30 report no pen tilt, so tilt can't drive the brush angle yet.
        let pressure = ui.input(|i| {
            i.raw.events.iter().rev().find_map(|e| match e {
                egui::Event::Touch { force: Some(f), .. } if f.is_finite() => Some(f.clamp(0.0, 1.0)),
                _ => None,
            })
        });
        let pressure = pressure.unwrap_or(1.0);
        if resp.drag_started_by(PointerButton::Primary) || resp.clicked_by(PointerButton::Primary) {
            // Shift+click paints a straight line from where the last stroke ended.
            let line_from = if shift { app.last_paint_point } else { None };
            if let Some(p) = resp.interact_pointer_pos() {
                match app.session.begin_stroke(line_from.unwrap_or(local(p)), app.session.viewport, pressure) {
                    Ok(_) => {
                        if line_from.is_some() {
                            app.session.continue_stroke(local(p), pressure);
                        }
                        app.last_paint_point = Some(local(p));
                    }
                    Err(e) => app.toast(e.to_string(), true),
                }
            }
            if resp.clicked_by(PointerButton::Primary)
                && let Err(e) = app.session.end_stroke()
            {
                app.toast(e.to_string(), true);
            }
        } else if resp.dragged_by(PointerButton::Primary)
            && app.session.stroke_active()
            && let Some(p) = resp.interact_pointer_pos()
        {
            app.session.continue_stroke(local(p), pressure);
            app.last_paint_point = Some(local(p));
        }
        if resp.drag_stopped_by(PointerButton::Primary)
            && let Err(e) = app.session.end_stroke()
        {
            app.toast(e.to_string(), true);
        }
    }

    let frame = Frame {
        view_id: crate::viewport::VIEW_3D,
        size: [(size.x * ui.ctx().pixels_per_point()).max(1.0) as u32, (size.y * ui.ctx().pixels_per_point()).max(1.0) as u32],
        kind: ViewKind::Scene,
        camera: app.session.camera,
        shade: shade(app),
        visible: app.session.doc.project.texture_sets.iter().map(|t| t.visible).collect(),
    };
    add_callback(app, ui, rect, frame, sync);
    // Shelf drops: a material goes above the selection (as in Painter), an alpha on the brush.
    crate::shelf::drop_target(app, ui, &resp, crate::shelf::DropZone::View3d);

    // Brush cursor.
    if app.ui.mode == crate::state::Mode::Painting
        && !navigating
        && resp.hovered()
        && matches!(app.session.tool.tool, Tool::Paint | Tool::Eraser | Tool::Projection | Tool::Smudge | Tool::Clone | Tool::Path)
        && let Some(hp) = resp.hover_pos()
        && let Some(hit) = app.session.pick(local(hp), app.session.viewport)
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::None);
        brush_cursor(app, ui, rect, hit.pos, hit.nrm);
        if app.session.tool.tool == Tool::Clone
            && let Some(src) = app.session.clone_source_point(Some(hit.uv))
        {
            clone_marker(app, ui, rect, src);
        }
        // Shift: preview the straight line Shift+click paints from the last stroke.
        if shift
            && !path_tool
            && let Some(from) = app.last_paint_point
        {
            rubber_band(ui, rect, pos2(rect.left() + from.x, rect.top() + from.y), hp);
        }
    }
    if app.ui.mode == crate::state::Mode::Painting && path_tool {
        path_overlay(app, ui, rect, &resp, false, show);
    }
    stencil_overlay(app, ui, rect);
    decal_overlay(app, ui, rect);
    if app.session.quick_mask.editing {
        ui.painter_at(rect).text(
            rect.left_top() + vec2(12.0, 40.0),
            egui::Align2::LEFT_TOP,
            "Editing quick mask: paint where painting is allowed (U to finish)",
            egui::FontId::proportional(12.0),
            Color32::from_rgb(0xe8, 0x6a, 0x6a),
        );
    }
    view_header(app, ui, rect, "v3d");
    axis_gizmo(ui, rect, app, false);
}

/// The frame of the selected decal layer over the 3D view (a hint while placing it): outline,
/// a tick on its top edge and its projection direction.
fn decal_overlay(app: &mut TextureCraftApp, ui: &Ui, rect: Rect) {
    use texturecraft_engine::core::{Fill, LayerKind, Projection};
    let p = &app.session.doc.project;
    let sel = app.session.selected_layer().and_then(|id| p.content_layer(id));
    let decal = match sel.map(|l| &l.kind) {
        Some(LayerKind::Fill(Fill { projection: Projection::Decal(d), .. })) => Some(*d),
        _ => None,
    };
    // Placing follows the selection: another layer selected stops it.
    if app.placing_decal.is_some() && app.placing_decal != sel.map(|l| l.id).filter(|_| decal.is_some()) {
        app.placing_decal = None;
    }
    let painter = ui.painter_at(rect);
    if app.placing_decal.is_some() {
        let hint = "Click the mesh to place the decal";
        painter.text(rect.left_top() + vec2(12.0, 40.0), egui::Align2::LEFT_TOP, hint, egui::FontId::proportional(12.0), TOKENS.accent);
    }
    let Some((d, corners)) = decal.and_then(|d| Some((d, texturecraft_engine::compose::pattern::decal_corners(&d)?))) else { return };
    let pts: Vec<Pos2> = corners.iter().filter_map(|c| project(app, rect, *c)).collect();
    let (Some(axes), true) = (texturecraft_engine::compose::pattern::decal_axes(&d), pts.len() == 4) else { return };
    let mut outline = pts.clone();
    outline.extend(pts.first().copied());
    for (w, color) in [(3.0, Color32::from_black_alpha(140)), (1.2, Color32::from_white_alpha(220))] {
        painter.add(egui::Shape::dashed_line(&outline, Stroke::new(w, color), 6.0, 4.0));
    }
    // Top edge tick and the projection direction from the center.
    let (o, _, up, n) = axes;
    let top = (corners[0] + corners[1]) * 0.5;
    let reach = d.size[0].max(d.size[1]) * 0.25;
    for (a, b) in [(top, top + up * reach * 0.4), (o, o + n * reach)] {
        if let (Some(a), Some(b)) = (project(app, rect, a), project(app, rect, b)) {
            painter.line_segment([a, b], Stroke::new(3.0, Color32::from_black_alpha(140)));
            painter.line_segment([a, b], Stroke::new(1.4, TOKENS.accent));
        }
    }
    if let Some(c) = project(app, rect, o) {
        painter.circle(c, 3.5, TOKENS.accent, Stroke::new(1.0, Color32::from_black_alpha(180)));
    }
}

/// Draw the projection stencil over the 3D view.
fn stencil_overlay(app: &mut TextureCraftApp, ui: &Ui, rect: Rect) {
    if app.session.tool.tool != Tool::Projection {
        return;
    }
    let Some(st) = app.session.tool.stencil.clone() else { return };
    let Some(img) = app.session.images.get(&st.image).cloned() else { return };
    let tex = app.thumbs.stencil(ui.ctx(), &st.image, &img);
    let aspect = rect.width() / rect.height().max(1.0);
    let (center, half, rot) = st.placement(img.width as f32 / img.height.max(1) as f32, aspect);
    let to_screen = |nx: f32, ny: f32| pos2(rect.left() + (nx * 0.5 + 0.5) * rect.width(), rect.top() + (0.5 - ny * 0.5) * rect.height());
    let (sn, cs) = rot.sin_cos();
    let corner = |lx: f32, ly: f32| {
        let (x, y) = (lx * half[0], ly * half[1]);
        to_screen(center[0] + x * cs - y * sn, center[1] + x * sn + y * cs)
    };
    let tint = Color32::from_white_alpha((st.opacity * 255.0) as u8);
    let mut mesh = egui::Mesh::with_texture(tex.id());
    for (lx, ly, u, v) in [(-1.0, 1.0, 0.0, 0.0), (1.0, 1.0, 1.0, 0.0), (1.0, -1.0, 1.0, 1.0), (-1.0, -1.0, 0.0, 1.0)] {
        mesh.vertices.push(egui::epaint::Vertex { pos: corner(lx, ly), uv: pos2(u, v), color: tint });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    let p = ui.painter_at(rect);
    p.add(egui::Shape::mesh(mesh));
    let outline = vec![corner(-1.0, 1.0), corner(1.0, 1.0), corner(1.0, -1.0), corner(-1.0, -1.0), corner(-1.0, 1.0)];
    p.add(egui::Shape::dashed_line(&outline, Stroke::new(1.0, Color32::from_white_alpha(120)), 5.0, 4.0));
}

/// A world point on screen in the 3D view `rect` (None behind the camera).
fn project(app: &TextureCraftApp, rect: Rect, p: Vec3) -> Option<Pos2> {
    let n = app.session.camera.project(p, rect.width() / rect.height().max(1.0))?;
    Some(pos2(rect.left() + (n.x * 0.5 + 0.5) * rect.width(), rect.top() + (0.5 - n.y * 0.5) * rect.height()))
}

/// Control point grab radius (points).
const HANDLE_GRAB: f32 = 8.0;

/// Path tool keys: Enter applies the path, Backspace removes its last point (Esc, handled with
/// the global shortcuts, cancels it).
fn path_keys(app: &mut TextureCraftApp, ui: &Ui) {
    if app.ui.mode != crate::state::Mode::Painting
        || app.session.tool.tool != Tool::Path
        || app.session.tool.path.points.is_empty()
        || ui.ctx().egui_wants_keyboard_input()
    {
        return;
    }
    let (enter, back) = ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::Enter), i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)));
    if enter {
        app.run("tool.path", json!({"commit": true}));
    }
    if back {
        app.session.tool.path.points.pop();
    }
}

/// Path tool input in a view (`uv`: the 2D view). Pressing on a control point drags it,
/// elsewhere adds a point (and drags it); clicking the first point of an open path closes it.
/// `place` maps a pointer position to a path point (None off the surface), `show` a path point
/// to the screen.
fn path_input(
    app: &mut TextureCraftApp,
    ui: &Ui,
    resp: &egui::Response,
    uv: bool,
    place: impl Fn(&TextureCraftApp, Pos2) -> Option<Vec3>,
    show: impl Fn(&TextureCraftApp, Vec3) -> Option<Pos2>,
) {
    let drag_id = resp.id.with("path_drag");
    let clicked = resp.clicked_by(PointerButton::Primary);
    if (clicked || resp.drag_started_by(PointerButton::Primary))
        && let Some(at) = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
    {
        let path = &app.session.tool.path;
        let near = if path.uv == uv { path.points.iter().position(|p| show(app, *p).is_some_and(|s| s.distance(at) < HANDLE_GRAB)) } else { None };
        match near {
            Some(0) if clicked && !path.closed && path.points.len() >= 3 => app.session.tool.path.closed = true,
            Some(i) => ui.data_mut(|d| {
                d.insert_temp(drag_id, i);
            }),
            None => match place(app, at) {
                Some(p) => {
                    app.session.tool.path.push(p, uv);
                    let last = app.session.tool.path.points.len().saturating_sub(1);
                    ui.data_mut(|d| {
                        d.insert_temp(drag_id, last);
                    });
                }
                None => app.toast("Place path points on the mesh", false),
            },
        }
    }
    if resp.dragged_by(PointerButton::Primary)
        && let Some(i) = ui.data(|d| d.get_temp::<usize>(drag_id))
        && let Some(p) = resp.interact_pointer_pos().and_then(|pos| place(app, pos))
        && let Some(q) = app.session.tool.path.points.get_mut(i)
    {
        *q = p;
    }
    if !resp.is_pointer_button_down_on() {
        ui.data_mut(|d| d.remove::<usize>(drag_id));
    }
}

/// The path being edited: its curve, control points (the first one in the accent color) and,
/// while open, a dashed preview of the next segment.
fn path_overlay(app: &TextureCraftApp, ui: &Ui, rect: Rect, resp: &egui::Response, uv: bool, show: impl Fn(&TextureCraftApp, Vec3) -> Option<Pos2>) {
    let p = ui.painter_at(rect);
    let path = &app.session.tool.path;
    let hint = "Path: click adds a point, drag moves it, Enter applies, Esc cancels";
    p.text(rect.left_bottom() + vec2(56.0, -12.0), egui::Align2::LEFT_BOTTOM, hint, egui::FontId::proportional(12.0), TOKENS.text_dim);
    if path.uv != uv {
        return;
    }
    let handles: Vec<Pos2> = path.points.iter().filter_map(|q| show(app, *q)).collect();
    let pts: Vec<Vec2> = handles.iter().map(|h| Vec2::new(h.x, h.y)).collect();
    let curve: Vec<Pos2> = texturecraft_engine::paint::spline(&pts, path.closed, 3.0).into_iter().map(|v| pos2(v.x, v.y)).collect();
    let color = if path.closed && path.fill { TOKENS.accent } else { Color32::from_white_alpha(230) };
    p.add(egui::Shape::line(curve.clone(), Stroke::new(3.0, Color32::from_black_alpha(140))));
    p.add(egui::Shape::line(curve, Stroke::new(1.4, color)));
    let hover = resp.hover_pos();
    if !path.closed
        && let (Some(last), Some(to)) = (handles.last(), hover)
    {
        rubber_band(ui, rect, *last, to);
    }
    for (i, h) in handles.iter().enumerate() {
        let r = if hover.is_some_and(|m| m.distance(*h) < HANDLE_GRAB) { 5.5 } else { 4.0 };
        p.circle(*h, r, if i == 0 { TOKENS.accent } else { Color32::WHITE }, Stroke::new(1.5, Color32::from_black_alpha(180)));
    }
}

/// Dashed preview of a straight segment.
fn rubber_band(ui: &Ui, rect: Rect, from: Pos2, to: Pos2) {
    let p = ui.painter_at(rect);
    p.add(egui::Shape::dashed_line(&[from, to], Stroke::new(2.5, Color32::from_black_alpha(120)), 6.0, 4.0));
    p.add(egui::Shape::dashed_line(&[from, to], Stroke::new(1.0, Color32::from_white_alpha(200)), 6.0, 4.0));
}

/// V held with the Clone tool in painting mode: clicks set the clone source.
fn clone_source_key(app: &TextureCraftApp, ui: &Ui) -> bool {
    app.ui.mode == crate::state::Mode::Painting
        && app.session.tool.tool == Tool::Clone
        && ui.input(|i| i.key_down(egui::Key::V))
        && !ui.ctx().egui_wants_keyboard_input()
}

/// Crosshair where the Clone tool reads from.
fn clone_marker(app: &TextureCraftApp, ui: &Ui, rect: Rect, pos: Vec3) {
    let Some(c) = project(app, rect, pos) else { return };
    let p = ui.painter_at(rect);
    for (w, color) in [(3.0, Color32::from_black_alpha(140)), (1.2, TOKENS.accent)] {
        p.circle_stroke(c, 7.0, Stroke::new(w, color));
        p.line_segment([c - vec2(11.0, 0.0), c + vec2(11.0, 0.0)], Stroke::new(w, color));
        p.line_segment([c - vec2(0.0, 11.0), c + vec2(0.0, 11.0)], Stroke::new(w, color));
    }
}

fn brush_cursor(app: &TextureCraftApp, ui: &Ui, rect: Rect, pos: Vec3, nrm: Vec3) {
    let (lo, hi) = app.session.mesh.bounds();
    let r = app.session.tool.brush.size * (hi - lo).length();
    let t = nrm.any_orthonormal_vector();
    let b = nrm.cross(t);
    let to_screen = |p: Vec3| project(app, rect, p);
    let mut pts = Vec::with_capacity(49);
    for k in 0..=48 {
        let a = k as f32 / 48.0 * std::f32::consts::TAU;
        if let Some(s) = to_screen(pos + (t * a.cos() + b * a.sin()) * r) {
            pts.push(s);
        }
    }
    let p = ui.painter_at(rect);
    p.add(egui::Shape::line(pts.clone(), Stroke::new(2.5, Color32::from_black_alpha(140))));
    p.add(egui::Shape::line(pts, Stroke::new(1.2, Color32::from_white_alpha(230))));
    let h = app.session.tool.brush.hardness;
    if h > 0.05 {
        let mut inner = Vec::new();
        for k in 0..=48 {
            let a = k as f32 / 48.0 * std::f32::consts::TAU;
            if let Some(s) = to_screen(pos + (t * a.cos() + b * a.sin()) * r * h) {
                inner.push(s);
            }
        }
        p.add(egui::Shape::dashed_line(&inner, Stroke::new(1.0, Color32::from_white_alpha(120)), 3.0, 3.0));
    }
    if let Some(c) = to_screen(pos) {
        p.circle_filled(c, 1.5, Color32::WHITE);
    }
}

fn view_2d(app: &mut TextureCraftApp, ui: &mut Ui, rect: Rect, sync: &mut PendingSync) {
    let resp = ui.interact(rect, ui.id().with("view2d"), Sense::click_and_drag());
    let size = rect.size();
    let alt = ui.input(|i| i.modifiers.alt);
    let delta = ui.input(|i| i.pointer.delta());
    if resp.dragged_by(PointerButton::Middle) || (alt && resp.dragged_by(PointerButton::Primary)) {
        app.ui.uv_pan[0] += delta.x / size.x * 2.0;
        app.ui.uv_pan[1] -= delta.y / size.y * 2.0;
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            let old = app.ui.uv_zoom;
            let new = (old * (1.0 + scroll * 0.002)).clamp(0.1, 40.0);
            // Zoom about the cursor.
            if let Some(hp) = resp.hover_pos() {
                let c = [(hp.x - rect.left()) / size.x * 2.0 - 1.0, 1.0 - (hp.y - rect.top()) / size.y * 2.0];
                for k in 0..2 {
                    app.ui.uv_pan[k] = c[k] - (c[k] - app.ui.uv_pan[k]) * new / old;
                }
            }
            app.ui.uv_zoom = new;
        }
    }
    let ts = app.session.ts();
    let set = app.session.doc.project.texture_sets.get(ts);
    let [sx, sy] = texturecraft_render::uv_view_scale(size.x / size.y.max(1.0), app.ui.uv_zoom, set.map_or((1, 1), |t| t.size()));
    let pan = app.ui.uv_pan;
    let to_uv = |p: egui::Pos2| {
        let xc = (p.x - rect.left()) / size.x * 2.0 - 1.0;
        let yc = 1.0 - (p.y - rect.top()) / size.y * 2.0;
        Vec2::new(((xc - pan[0]) / sx + 1.0) * 0.5, (1.0 - (yc - pan[1]) / sy) * 0.5)
    };
    let to = |u: f32, v: f32| {
        let x = (u * 2.0 - 1.0) * sx + pan[0];
        let y = (1.0 - v * 2.0) * sy + pan[1];
        pos2(rect.left() + (x * 0.5 + 0.5) * size.x, rect.top() + (0.5 - y * 0.5) * size.y)
    };
    let show = |_: &TextureCraftApp, p: Vec3| Some(to(p.x, p.y));
    let path_tool = app.session.tool.tool == Tool::Path;
    // Clone source (V + click) and painting in UV space.
    let v_held = clone_source_key(app, ui);
    if v_held
        && (resp.clicked_by(PointerButton::Primary) || resp.drag_started_by(PointerButton::Primary))
        && let Some(p) = resp.interact_pointer_pos()
    {
        let uv = to_uv(p);
        app.run("tool.cloneSource", json!({"uv": [uv.x, uv.y]}));
    }
    let painting = app.ui.mode == crate::state::Mode::Painting && !alt && !v_held;
    if painting && path_tool {
        path_input(app, ui, &resp, true, |_, p| Some(to_uv(p).extend(0.0)), show);
    } else if painting {
        if resp.drag_started_by(PointerButton::Primary) || resp.clicked_by(PointerButton::Primary) {
            if let Some(p) = resp.interact_pointer_pos()
                && let Err(e) = app.session.begin_stroke_uv(to_uv(p), 1.0)
            {
                app.toast(e.to_string(), true);
            }
            if resp.clicked_by(PointerButton::Primary)
                && let Err(e) = app.session.end_stroke()
            {
                app.toast(e.to_string(), true);
            }
        } else if resp.dragged_by(PointerButton::Primary)
            && app.session.stroke_active()
            && let Some(p) = resp.interact_pointer_pos()
        {
            app.session.continue_stroke_uv(to_uv(p), 1.0);
        }
        if resp.drag_stopped_by(PointerButton::Primary)
            && let Err(e) = app.session.end_stroke()
        {
            app.toast(e.to_string(), true);
        }
    }
    if resp.double_clicked() && !(painting && path_tool) {
        app.ui.uv_zoom = 1.0;
        app.ui.uv_pan = [0.0, 0.0];
    }
    let frame = Frame {
        view_id: VIEW_2D,
        size: [(size.x * ui.ctx().pixels_per_point()).max(1.0) as u32, (size.y * ui.ctx().pixels_per_point()).max(1.0) as u32],
        kind: ViewKind::Uv { ts, zoom: app.ui.uv_zoom, pan: app.ui.uv_pan },
        camera: app.session.camera,
        shade: shade(app),
        visible: app.session.doc.project.texture_sets.iter().map(|t| t.visible).collect(),
    };
    add_callback(app, ui, rect, frame, sync);
    crate::shelf::drop_target(app, ui, &resp, crate::shelf::DropZone::View2d);
    // The UV tile outlines (numbered for UDIM sets) and the path being edited.
    let painter = ui.painter_at(rect);
    let set = app.session.doc.project.texture_sets.get(ts);
    let outline = |r: Rect| painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_white_alpha(40)), egui::StrokeKind::Outside);
    match set.and_then(|t| Some((t, t.tile_grid()?))) {
        Some((t, g)) => {
            for (udim, (bx, by)) in t.uv_tiles.iter().filter_map(|u| Some((*u, g.block(*u)?))) {
                let (u0, v0) = (bx as f32 / g.cols as f32, by as f32 / g.rows as f32);
                let r = Rect::from_two_pos(to(u0, v0), to(u0 + 1.0 / g.cols as f32, v0 + 1.0 / g.rows as f32));
                outline(r);
                painter.text(r.left_top() + vec2(6.0, 4.0), egui::Align2::LEFT_TOP, udim.to_string(), egui::FontId::proportional(11.0), TOKENS.text_dim);
            }
        }
        None => {
            outline(Rect::from_two_pos(to(0.0, 0.0), to(1.0, 1.0)));
        }
    }
    if app.ui.mode == crate::state::Mode::Painting && path_tool {
        path_overlay(app, ui, rect, &resp, true, show);
    }
    view_header(app, ui, rect, "v2d");
    axis_gizmo(ui, rect, app, true);
    let name = app.session.doc.project.texture_sets.get(ts).map(|t| t.name.clone()).unwrap_or_default();
    ui.painter().text(rect.right_bottom() + vec2(-10.0, -10.0), egui::Align2::RIGHT_BOTTOM, name, egui::FontId::proportional(11.0), TOKENS.text_dim);
    let _ = (RichText::new(""), Channel::BaseColor);
}
