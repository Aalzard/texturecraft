//! Compact widgets matching the app's look: labelled slim sliders, icon buttons, section
//! headers, channel toggle chips and color swatches.

use egui::{Color32, Response, RichText, Sense, Stroke, Ui, vec2};

use crate::theme::TOKENS;

/// The drag-and-drop payload of type `T` released on `resp` this frame. Unlike
/// [`Response::dnd_release_payload`] (which clears a payload of any type), it leaves payloads
/// of other types for the next drop target.
pub fn dnd_release<T: std::any::Any + Send + Sync>(resp: &Response) -> Option<std::sync::Arc<T>> {
    resp.dnd_hover_payload::<T>().and_then(|_| resp.dnd_release_payload::<T>())
}

/// A labelled slider in two rows: "Label ........ value" over a slim track with a round knob.
/// Returns the response of the track (`.changed()` when dragged, `.drag_stopped()` at the end).
pub fn slider(ui: &mut Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, decimals: usize) -> Response {
    let (lo, hi) = (*range.start(), *range.end());
    let w = ui.available_width();
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(TOKENS.text));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut text = format!("{:.*}", decimals, value);
            let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(44.0).horizontal_align(egui::Align::RIGHT).frame(egui::Frame::NONE));
            if r.lost_focus()
                && let Ok(v) = text.trim().parse::<f32>()
                && v.is_finite()
            {
                *value = v.clamp(lo, hi);
            }
        });
    });
    let (rect, mut resp) = ui.allocate_exact_size(vec2(w, 14.0), Sense::click_and_drag());
    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let t = ((p.x - rect.left() - 5.0) / (rect.width() - 10.0).max(1.0)).clamp(0.0, 1.0);
        let v = lo + (hi - lo) * t;
        if (v - *value).abs() > f32::EPSILON {
            *value = v;
            resp.mark_changed();
        }
    }
    let p = ui.painter();
    let y = rect.center().y;
    let (x0, x1) = (rect.left() + 5.0, rect.right() - 5.0);
    let t = if hi > lo { ((*value - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 };
    let xk = x0 + (x1 - x0) * t;
    p.line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], Stroke::new(2.0, Color32::from_rgb(0x4a, 0x4a, 0x4a)));
    p.line_segment([egui::pos2(x0, y), egui::pos2(xk, y)], Stroke::new(2.0, Color32::from_rgb(0xb8, 0xb8, 0xb8)));
    let knob = if resp.hovered() || resp.dragged() { TOKENS.text_strong } else { Color32::from_rgb(0xc8, 0xc8, 0xc8) };
    p.circle(egui::pos2(xk, y), 4.5, Color32::from_rgb(0x26, 0x26, 0x26), Stroke::new(1.5, knob));
    resp
}

/// A small inline slider for the top tool bar: "Label value" above a short track.
pub fn bar_slider(ui: &mut Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, width: f32) -> Response {
    let (lo, hi) = (*range.start(), *range.end());
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click_and_drag());
    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let t = ((p.x - rect.left() - 4.0) / (rect.width() - 8.0).max(1.0)).clamp(0.0, 1.0);
        let v = lo + (hi - lo) * t;
        if (v - *value).abs() > f32::EPSILON {
            *value = v;
            resp.mark_changed();
        }
    }
    let p = ui.painter();
    let font = egui::FontId::proportional(11.0);
    p.text(rect.left_top() + vec2(2.0, 2.0), egui::Align2::LEFT_TOP, label, font.clone(), TOKENS.text);
    p.text(rect.right_top() + vec2(-2.0, 2.0), egui::Align2::RIGHT_TOP, format!("{:.0}", value), font, TOKENS.text_strong);
    let y = rect.bottom() - 7.0;
    let (x0, x1) = (rect.left() + 4.0, rect.right() - 4.0);
    let t = if hi > lo { ((*value - lo) / (hi - lo)).clamp(0.0, 1.0) } else { 0.0 };
    let xk = x0 + (x1 - x0) * t;
    p.line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], Stroke::new(1.5, Color32::from_rgb(0x4a, 0x4a, 0x4a)));
    p.line_segment([egui::pos2(x0, y), egui::pos2(xk, y)], Stroke::new(1.5, Color32::from_rgb(0xc0, 0xc0, 0xc0)));
    p.circle(egui::pos2(xk, y), 4.0, Color32::from_rgb(0x22, 0x22, 0x22), Stroke::new(1.5, Color32::from_rgb(0xd0, 0xd0, 0xd0)));
    resp
}

/// One line of `text` cut with "…" to fit `max_width`, for list rows that shrink with their panel.
pub fn elided(painter: &egui::Painter, text: impl Into<String>, size: f32, color: Color32, max_width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.into(), egui::FontId::proportional(size), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(0.0));
    painter.layout_job(job)
}

/// A square icon button (Phosphor glyph). `selected` draws the accent background.
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str, selected: bool, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let p = ui.painter();
    let bg = if selected {
        Color32::from_rgb(0x3d, 0x3d, 0x3d)
    } else if resp.hovered() {
        Color32::from_rgb(0x33, 0x33, 0x33)
    } else {
        Color32::TRANSPARENT
    };
    p.rect_filled(rect, 3.0, bg);
    if selected {
        p.rect_stroke(rect, 3.0, Stroke::new(1.0, Color32::from_rgb(0x55, 0x55, 0x55)), egui::StrokeKind::Inside);
    }
    let color = if selected || resp.hovered() { TOKENS.text_strong } else { TOKENS.text };
    p.text(rect.center(), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(size * 0.62), color);
    resp.on_hover_text(tooltip)
}

/// A collapsible section header with a chevron ("> Size").
pub fn section(ui: &mut Ui, id: &str, title: &str, default_open: bool, body: impl FnOnce(&mut Ui)) {
    let id = ui.make_persistent_id(id);
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, default_open);
    let open = state.is_open();
    let resp = ui
        .horizontal(|ui| {
            let chev = if open { egui_phosphor::regular::CARET_DOWN } else { egui_phosphor::regular::CARET_RIGHT };
            ui.label(RichText::new(chev).size(10.0).color(TOKENS.text_dim));
            ui.label(RichText::new(title).strong().color(TOKENS.text_strong));
        })
        .response
        .interact(Sense::click());
    if resp.clicked() {
        state.toggle(ui);
    }
    state.show_body_unindented(ui, |ui| {
        ui.indent(id.with("in"), body);
    });
}

/// A Painter-style channel chip ("color", "metal", "rough"…): toggled = accent outline.
pub fn chip(ui: &mut Ui, text: &str, on: bool) -> Response {
    let galley = ui.painter().layout_no_wrap(text.into(), egui::FontId::proportional(11.0), TOKENS.text);
    let size = vec2(galley.size().x + 14.0, 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, if on { Color32::from_rgb(0x3a, 0x3a, 0x3a) } else { Color32::from_rgb(0x2a, 0x2a, 0x2a) });
    p.rect_stroke(
        rect,
        3.0,
        Stroke::new(1.0, if on { Color32::from_rgb(0x9a, 0x9a, 0x9a) } else { Color32::from_rgb(0x3a, 0x3a, 0x3a) }),
        egui::StrokeKind::Inside,
    );
    p.text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(11.0), if on { TOKENS.text_strong } else { TOKENS.text_dim });
    resp
}

/// A color swatch button for a linear RGB value; opens a picker on click. Returns true when
/// the color changed.
pub fn color_swatch(ui: &mut Ui, id: &str, linear: &mut [f32; 3], width: f32) -> bool {
    let srgb = linear.map(|v| (texturecraft_engine::texel::linear_to_srgb(v) * 255.0 + 0.5) as u8);
    let mut c = Color32::from_rgb(srgb[0], srgb[1], srgb[2]);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 18.0), Sense::click());
    ui.painter().rect_filled(rect, 2.0, c);
    ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.0, Color32::from_rgb(0x55, 0x55, 0x55)), egui::StrokeKind::Inside);
    let popup = ui.make_persistent_id(id);
    if resp.clicked() {
        egui::Popup::toggle_id(ui.ctx(), popup);
    }
    let mut changed = false;
    egui::Popup::from_response(&resp).id(popup).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        if egui::color_picker::color_picker_color32(ui, &mut c, egui::color_picker::Alpha::Opaque) {
            let lin = [c.r(), c.g(), c.b()].map(|v| texturecraft_engine::texel::srgb_to_linear(v as f32 / 255.0));
            *linear = lin;
            changed = true;
        }
    });
    changed
}

/// Panel title row with optional trailing content.
pub fn panel_header(ui: &mut Ui, title: &str, trailing: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        ui.label(crate::theme::caption(title));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), trailing);
    });
}
